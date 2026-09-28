#!/usr/bin/env bash
# Run only on the disposable Monk builder, never the Station runtime host.
set -euo pipefail
[[ "${GITHUB_ACTIONS:-}" == true && "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]]
: "${GITHUB_SHA:?}" "${RUNNER_TEMP:?}"
cd "$(git rev-parse --show-toplevel)"
[[ "$(git rev-parse HEAD)" == "$GITHUB_SHA" ]]
git diff --exit-code HEAD -- .sqlx
[[ -z "$(git ls-files --others --exclude-standard .sqlx)" ]]
[[ -n "$(git ls-files '.sqlx/query-*.json')" ]]
for tool in cargo readelf ldd nix-store sha256sum tar; do command -v "$tool" >/dev/null; done
export SQLX_OFFLINE=true
unset RUSTC_WRAPPER
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$RUNNER_TEMP/station-m365-offline-target}"
cargo build --locked -p authentication_service -p email_service -p email_refresh_handler \
  --features email_service/microsoft_graph_readonly \
  --bin authentication_service --bin email_service --bin pubsub_workers --bin email_refresh_handler
out="$RUNNER_TEMP/station-mailbox-release"
mkdir -p "$out"
stage="$(mktemp -d "$RUNNER_TEMP/station-mailbox-stage.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/bin"
: > "$stage/roots"
for binary in authentication_service email_service pubsub_workers email_refresh_handler; do
  install -m 0755 "$CARGO_TARGET_DIR/debug/$binary" "$stage/bin/$binary"
  interpreter="$(readelf -l "$stage/bin/$binary" | sed -n 's/.*Requesting program interpreter: \([^]]*\)].*/\1/p')"
  [[ "$interpreter" == /nix/store/* && -f "$interpreter" ]]
  ldd "$stage/bin/$binary" > "$stage/$binary.ldd"
  ! grep -q 'not found' "$stage/$binary.ldd"
  printf '%s\n' "$interpreter" >> "$stage/paths"
  awk '{for(i=1;i<=NF;i++) if($i ~ /^\//) print $i}' "$stage/$binary.ldd" >> "$stage/paths"
done
while IFS= read -r dependency; do
  [[ "$dependency" == /nix/store/* && -e "$dependency" ]]
  root="${dependency#/nix/store/}"; root="/nix/store/${root%%/*}"
  printf '%s\n' "$root" >> "$stage/roots"
done < "$stage/paths"
mapfile -t roots < <(sort -u "$stage/roots")
[[ "${#roots[@]}" -gt 0 ]]
nix-store --query --requisites "${roots[@]}" | sort -u > "$stage/closure"
[[ -s "$stage/closure" ]]
while IFS= read -r store_path; do
  [[ "$store_path" =~ ^/nix/store/[a-z0-9]{32}-[^/]+$ && -e "$store_path" ]]
  [[ "$store_path" != *'/../'* && "$store_path" != *$'\n'* ]]
done < "$stage/closure"
sed 's@^/@@' "$stage/closure" > "$stage/closure-relative"
(
  cd "$stage"
  sha256sum bin/* > binaries.sha256
  printf '{"commit":"%s","profile":"dev","sqlx_offline":true,"feature":"email_service/microsoft_graph_readonly","runner":"monkci-ubuntu-24.04-8"}\n' "$GITHUB_SHA" > provenance.json
  tar -czf "$out/station-mailbox-binaries.tar.gz" bin binaries.sha256 provenance.json
)
tar -C / -czf "$out/station-mailbox-nix-closure.tar.gz" --files-from "$stage/closure-relative"
cp "$stage/provenance.json" "$stage/binaries.sha256" "$out/"
(cd "$out" && sha256sum ./*.tar.gz > archives.sha256)
printf 'Packaged commit %s; runtime closure paths: %s\n' "$GITHUB_SHA" "$(wc -l < "$stage/closure")"
