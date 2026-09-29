#!/usr/bin/env bash
# Disposable CI runner only. No runtime deployment, provider credentials or live buckets.
set -euo pipefail
[[ "${GITHUB_ACTIONS:-}" == true && "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]]
: "${GITHUB_SHA:?}" "${RUNNER_TEMP:?}"
cd "$(git rev-parse --show-toplevel)"
[[ "$(git rev-parse HEAD)" == "$GITHUB_SHA" ]]
[[ -z "$(git status --porcelain)" ]]
# Unit checks use explicit in-memory configuration and synthetic signing fixtures.
# No production credentials, remote object reads/writes, or database regeneration.
unset RUSTC_WRAPPER DATABASE_URL
export SQLX_OFFLINE=true
export CARGO_TARGET_DIR="$RUNNER_TEMP/station-storage-target"
export AWS_EC2_METADATA_DISABLED=true
export AWS_CONFIG_FILE=/dev/null
export AWS_SHARED_CREDENTIALS_FILE=/dev/null
unset AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY AWS_SESSION_TOKEN AWS_PROFILE
unset LOCAL_AWS_URL LOCAL_AWS_PUBLIC_URL
unset OBJECT_STORAGE_MODE OBJECT_STORAGE_REGION OBJECT_STORAGE_ENDPOINT_URL
unset OBJECT_STORAGE_ACCESS_KEY_ID OBJECT_STORAGE_SECRET_ACCESS_KEY OBJECT_STORAGE_SESSION_TOKEN
printf '{"commit":"%s","runner":"monkci-ubuntu-24.04-8","sqlx_offline":true,"deployment":false,"live_provider_test":false}\n' "$GITHUB_SHA" > "$RUNNER_TEMP/station-storage-provenance.json"
cargo test --locked -p macro_aws_config --all-features 2>&1 | tee "$RUNNER_TEMP/station-storage-config-tests.log"
cargo check --locked -p document_storage_service -p static_file_service -p email_service -p document_upload_finalizer_handler --features email_service/microsoft_graph_readonly 2>&1 | tee "$RUNNER_TEMP/station-storage-consumers.log"
# Run only bounded, pure consumer regressions; no live services or database fixtures.
# Cargo succeeds when a filter matches zero tests, so explicitly reject that case.
focused_test() {
  "$@" 2>&1 | tee "$RUNNER_TEMP/station-storage-focused.log" | tee -a "$RUNNER_TEMP/station-storage-consumer-tests.log"
  grep -Eq 'test result: ok\. [1-9][0-9]* passed;' "$RUNNER_TEMP/station-storage-focused.log"
}
for filter in document_get_provider_is_opt_in_and_defaults_to_cloudfront direct_s3_document_get_keeps_the_raw_object_key_and_configured_expiry direct_get_presigning_passes_the_raw_object_key_to_the_sdk s3_writes_are_skipped_only_when_s3_itself_is_local; do
  focused_test cargo test --locked -p documents --lib "$filter"
done
focused_test cargo test --locked -p call --lib direct_s3_get_is_used_only_for_custom_or_local_s3
for filter in document_get_provider_is_opt_in_and_defaults_to_cloudfront reconstructed_docx_export_defaults_to_cloudfront_and_requires_explicit_s3 reconstructed_docx_key_handling_is_provider_specific; do
  focused_test cargo test --locked -p document_storage_service --bin document_storage_service "$filter"
done
for filter in attachment_get_provider_is_opt_in_and_defaults_to_cloudfront legacy_cloudfront_attachment_url_keeps_whole_key_encoding; do
  focused_test cargo test --locked -p email_service --bin email_service --features microsoft_graph_readonly "$filter"
done
git diff -- Cargo.lock > "$RUNNER_TEMP/station-storage-lock.diff"
git diff --exit-code -- Cargo.lock .sqlx
[[ -z "$(git ls-files --others --exclude-standard .sqlx)" ]]
