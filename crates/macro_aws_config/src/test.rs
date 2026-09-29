use super::*;

fn raw_config(mode: Option<&str>) -> RawObjectStorageConfig {
    RawObjectStorageConfig {
        mode: mode.map(str::to_owned),
        region: None,
        endpoint_url: None,
        access_key_id: None,
        secret_access_key: None,
        session_token: None,
    }
}

fn aws_config_with_static_credentials() -> ObjectStorageConfig {
    let mut raw = raw_config(Some("aws"));
    raw.region = Some("us-west-2".to_owned());
    raw.access_key_id = Some("object-access-id".to_owned());
    raw.secret_access_key = Some("object-secret-value".to_owned());
    raw.session_token = Some("object-session-token".to_owned());
    raw.try_into().expect("valid AWS object-storage config")
}

fn aws_config_with_default_chain() -> ObjectStorageConfig {
    let mut raw = raw_config(Some("aws"));
    raw.region = Some("us-east-2".to_owned());
    raw.try_into().expect("valid AWS default-chain config")
}

fn ambient_fields(
    access_key_id: Option<&str>,
    secret_access_key: Option<&str>,
    session_token: Option<&str>,
) -> AmbientAwsCredentialFields {
    AmbientAwsCredentialFields {
        access_key_id: access_key_id.is_some(),
        secret_access_key: secret_access_key.is_some(),
        session_token: session_token.is_some(),
    }
}

#[cfg(feature = "s3")]
fn block_on_no_network<F: std::future::Future>(future: F) -> F::Output {
    struct NoopWake;

    impl std::task::Wake for NoopWake {
        fn wake(self: std::sync::Arc<Self>) {}
    }

    let waker = std::task::Waker::from(std::sync::Arc::new(NoopWake));
    let mut context = std::task::Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);

    for _ in 0..1_000 {
        if let std::task::Poll::Ready(output) =
            std::future::Future::poll(future.as_mut(), &mut context)
        {
            return output;
        }
        std::thread::yield_now();
    }

    panic!("no-network future did not complete synchronously")
}

fn compatible_config() -> ObjectStorageConfig {
    let mut raw = raw_config(Some("s3-compatible"));
    raw.region = Some("us-west-004".to_owned());
    raw.endpoint_url = Some("https://s3.us-west-004.example.invalid".to_owned());
    raw.access_key_id = Some("provider-access-id".to_owned());
    raw.secret_access_key = Some("provider-secret-value".to_owned());
    raw.try_into()
        .expect("valid compatible object-storage config")
}

#[test]
fn legacy_is_the_exact_default_mode() {
    let config = ObjectStorageConfig::try_from(raw_config(None)).expect("legacy config");
    assert!(matches!(&config, ObjectStorageConfig::Legacy));
    assert!(!is_custom_s3_config(&config));

    let input = "http://static-file-storage.localstack:4566/file/pasted%20image.png?X-Amz-Signature=test&x-id=PutObject";
    assert_eq!(
        transform_browser_url_for_config(
            input,
            &config,
            Some("http://localstack:4566"),
            Some("http://localhost:29806"),
        ),
        "http://localhost:29806/static-file-storage/file/pasted%20image.png?X-Amz-Signature=test&x-id=PutObject"
    );
}

#[test]
fn provider_settings_require_an_explicit_mode() {
    let mut raw = raw_config(None);
    raw.region = Some("us-east-1".to_owned());

    assert_eq!(
        ObjectStorageConfig::try_from(raw),
        Err(ObjectStorageConfigError::ModeRequired {
            variable: OBJECT_STORAGE_REGION,
        })
    );
}

#[test]
fn explicit_legacy_rejects_provider_settings() {
    let mut raw = raw_config(Some("legacy"));
    raw.endpoint_url = Some("https://storage.example.invalid".to_owned());

    assert_eq!(
        ObjectStorageConfig::try_from(raw),
        Err(ObjectStorageConfigError::UnexpectedVariable {
            variable: OBJECT_STORAGE_ENDPOINT_URL,
            mode: "legacy",
        })
    );
}

#[test]
fn aws_region_and_credentials_are_independent_from_local_aws() {
    let config = aws_config_with_static_credentials();
    let input = "https://bucket.s3.us-west-2.amazonaws.com/a%2Fb%20c?X-Amz-Credential=object-access-id%2Fscope&X-Amz-Signature=abc%2B123";

    assert_eq!(
        transform_browser_url_for_config(
            input,
            &config,
            Some("http://localstack:4566"),
            Some("http://localhost:4566"),
        ),
        input
    );
    assert_eq!(
        transform_internal_url_for_config(input, &config, Some("http://localstack:4566")),
        input
    );

    match config {
        ObjectStorageConfig::Aws {
            region,
            credentials: ObjectStorageCredentials::Static { .. },
        } => assert_eq!(region, "us-west-2"),
        _ => panic!("expected AWS static configuration"),
    }
}

#[test]
fn aws_mode_supports_the_default_credential_chain() {
    let config = aws_config_with_default_chain();

    assert!(matches!(
        &config,
        ObjectStorageConfig::Aws {
            credentials: ObjectStorageCredentials::DefaultChain,
            ..
        }
    ));
    assert!(is_custom_s3_config(&config));
}

#[test]
fn aws_default_chain_rejects_any_ambient_static_credentials_when_local_aws_is_set() {
    let config = aws_config_with_default_chain();
    let ambient_values = [
        (Some("ambient-access-value"), None, None),
        (None, Some("ambient-secret-value"), None),
        (None, None, Some("ambient-session-value")),
        (
            Some("ambient-access-value"),
            Some("ambient-secret-value"),
            Some("ambient-session-value"),
        ),
    ];

    for (access_key_id, secret_access_key, session_token) in ambient_values {
        let error = validate_default_chain_isolation(
            &config,
            true,
            ambient_fields(access_key_id, secret_access_key, session_token),
        )
        .expect_err("ambient development credentials must fail closed");
        let rendered = format!("{error:?} {error}");
        assert_eq!(
            error,
            ObjectStorageConfigError::AmbientAwsCredentialsWithLocalEndpoint
        );
        for value in [access_key_id, secret_access_key, session_token]
            .into_iter()
            .flatten()
        {
            assert!(!rendered.contains(value));
        }
        assert!(rendered.contains("use an IAM role"));
        assert!(rendered.contains("dedicated OBJECT_STORAGE credentials"));
    }
}

#[test]
fn dedicated_object_storage_credentials_are_allowed_with_local_aws_and_ambient_credentials() {
    let config = aws_config_with_static_credentials();
    validate_default_chain_isolation(
        &config,
        true,
        ambient_fields(
            Some("ambient-access-value"),
            Some("ambient-secret-value"),
            Some("ambient-session-value"),
        ),
    )
    .expect("dedicated object-storage credentials prevent cross-use");

    match config {
        ObjectStorageConfig::Aws {
            credentials:
                ObjectStorageCredentials::Static {
                    access_key_id,
                    secret_access_key,
                    session_token,
                },
            ..
        } => {
            assert_eq!(access_key_id, "object-access-id");
            assert_eq!(secret_access_key, "object-secret-value");
            assert_eq!(session_token.as_deref(), Some("object-session-token"));
        }
        _ => panic!("expected dedicated AWS object-storage credentials"),
    }
}

#[test]
fn aws_default_chain_allows_iam_when_local_aws_has_no_ambient_static_credentials() {
    validate_default_chain_isolation(
        &aws_config_with_default_chain(),
        true,
        AmbientAwsCredentialFields::default(),
    )
    .expect("IAM and the non-static default chain remain available");
}

#[test]
fn ambient_static_credentials_without_local_aws_do_not_change_remote_aws_behavior() {
    validate_default_chain_isolation(
        &aws_config_with_default_chain(),
        false,
        ambient_fields(
            Some("ambient-access-value"),
            Some("ambient-secret-value"),
            Some("ambient-session-value"),
        ),
    )
    .expect("the guard is scoped to a simultaneous local AWS endpoint");
}

#[test]
fn legacy_mode_ignores_ambient_static_credentials() {
    let legacy = ObjectStorageConfig::try_from(raw_config(None)).expect("legacy config");
    validate_default_chain_isolation(
        &legacy,
        true,
        ambient_fields(
            Some("ambient-access-value"),
            Some("ambient-secret-value"),
            Some("ambient-session-value"),
        ),
    )
    .expect("legacy behavior remains unchanged");
}

#[test]
fn aws_mode_rejects_a_custom_endpoint() {
    let mut raw = raw_config(Some("aws"));
    raw.region = Some("us-east-1".to_owned());
    raw.endpoint_url = Some("https://storage.example.invalid".to_owned());

    assert_eq!(
        ObjectStorageConfig::try_from(raw),
        Err(ObjectStorageConfigError::UnexpectedVariable {
            variable: OBJECT_STORAGE_ENDPOINT_URL,
            mode: "aws",
        })
    );
}

#[test]
fn compatible_mode_requires_tls_endpoint_and_static_credentials() {
    for endpoint in [
        "http://storage.example.invalid",
        "https://user:password@storage.example.invalid",
        "https://storage.example.invalid/base-path",
        "https://storage.example.invalid?token=secret",
    ] {
        let mut raw = raw_config(Some("s3-compatible"));
        raw.region = Some("provider-region-1".to_owned());
        raw.endpoint_url = Some(endpoint.to_owned());
        raw.access_key_id = Some("access".to_owned());
        raw.secret_access_key = Some("secret".to_owned());
        assert!(matches!(
            ObjectStorageConfig::try_from(raw),
            Err(ObjectStorageConfigError::InvalidValue {
                variable: OBJECT_STORAGE_ENDPOINT_URL,
                ..
            })
        ));
    }

    let mut missing_credentials = raw_config(Some("s3-compatible"));
    missing_credentials.region = Some("provider-region-1".to_owned());
    missing_credentials.endpoint_url = Some("https://storage.example.invalid".to_owned());
    assert_eq!(
        ObjectStorageConfig::try_from(missing_credentials),
        Err(ObjectStorageConfigError::MissingVariable {
            variable: OBJECT_STORAGE_ACCESS_KEY_ID,
            mode: "s3-compatible",
        })
    );
}

#[test]
fn partial_credentials_fail_closed_in_all_remote_modes() {
    for mode in ["aws", "s3-compatible"] {
        let mut raw = raw_config(Some(mode));
        raw.region = Some("us-east-1".to_owned());
        raw.endpoint_url =
            (mode == "s3-compatible").then(|| "https://storage.example.invalid".to_owned());
        raw.access_key_id = Some("only-access-key".to_owned());

        assert_eq!(
            ObjectStorageConfig::try_from(raw),
            Err(ObjectStorageConfigError::PartialCredentials)
        );
    }
}

#[test]
fn invalid_region_fails_without_echoing_the_value() {
    let invalid_region = "secret region value";
    let mut raw = raw_config(Some("aws"));
    raw.region = Some(invalid_region.to_owned());

    let error = ObjectStorageConfig::try_from(raw).expect_err("invalid region");
    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains(invalid_region));
}

#[test]
fn endpoint_errors_do_not_echo_embedded_sensitive_values() {
    let embedded_secret = "embedded-password-value";
    let mut raw = raw_config(Some("s3-compatible"));
    raw.region = Some("provider-region-1".to_owned());
    raw.endpoint_url = Some(format!(
        "https://user:{embedded_secret}@storage.example.invalid"
    ));
    raw.access_key_id = Some("access".to_owned());
    raw.secret_access_key = Some("secret".to_owned());

    let error = ObjectStorageConfig::try_from(raw).expect_err("userinfo must fail");
    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains(embedded_secret));
}

#[test]
fn config_debug_redacts_all_static_credential_values() {
    let config = aws_config_with_static_credentials();
    let rendered = format!("{config:?}");

    assert!(!rendered.contains("object-access-id"));
    assert!(!rendered.contains("object-secret-value"));
    assert!(!rendered.contains("object-session-token"));
    assert!(rendered.contains("Static(<redacted>)"));
}

#[cfg(feature = "s3")]
#[test]
fn sdk_configs_build_with_explicit_remote_region_and_credentials() {
    use aws_sdk_s3::config::{BehaviorVersion, Builder};

    for (config, expected_region) in [
        (aws_config_with_static_credentials(), "us-west-2"),
        (compatible_config(), "us-west-004"),
    ] {
        let sdk_config = apply_remote_s3_overrides(
            Builder::new().behavior_version(BehaviorVersion::latest()),
            config,
        )
        .build();
        assert_eq!(
            sdk_config.region().map(|region| region.as_ref()),
            Some(expected_region)
        );
        let _client = aws_sdk_s3::Client::from_conf(sdk_config);
    }
}

#[cfg(feature = "s3")]
#[test]
fn remote_aws_clears_generic_and_service_specific_endpoint_overrides() {
    use std::time::Duration;

    use aws_sdk_s3::{
        config::{BehaviorVersion, Builder, Credentials, Region, SharedCredentialsProvider},
        presigning::PresigningConfig,
    };

    let shared_config = SdkConfig::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-west-2"))
        .endpoint_url("https://ambient-generic-endpoint.invalid")
        .credentials_provider(SharedCredentialsProvider::new(Credentials::new(
            "endpoint-test-access",
            "endpoint-test-secret",
            None,
            None,
            "endpoint-test",
        )))
        .build();

    for service_specific_override in [
        None,
        Some("https://ambient-service-specific-endpoint.invalid"),
    ] {
        let mut builder = Builder::from(&shared_config);
        // The default lazy identity cache needs a Tokio timer; this synchronous,
        // no-network test uses static test credentials directly instead.
        builder.set_identity_cache(aws_sdk_s3::config::IdentityCache::no_cache());
        if let Some(endpoint) = service_specific_override {
            // `Builder::from` materializes `AWS_ENDPOINT_URL_S3` into this same
            // service-level field before our override helper runs.
            builder.set_endpoint_url(Some(endpoint.to_owned()));
        }

        let sdk_config =
            apply_remote_s3_overrides(builder, aws_config_with_static_credentials()).build();
        let client = aws_sdk_s3::Client::from_conf(sdk_config);
        let presigned = block_on_no_network(
            client
                .get_object()
                .bucket("endpoint-clear-test")
                .key("folder/a%2Fb")
                .presigned(
                    PresigningConfig::expires_in(Duration::from_secs(60))
                        .expect("valid presigning duration"),
                ),
        )
        .expect("presigning does not require network access");

        assert!(presigned.uri().contains("amazonaws.com"));
        assert!(!presigned.uri().contains("ambient-generic-endpoint"));
        assert!(
            !presigned
                .uri()
                .contains("ambient-service-specific-endpoint")
        );
    }
}

#[test]
fn remote_signed_urls_are_preserved_byte_for_byte() {
    let input = "https://bucket.storage.example.invalid/folder/a%2Fb%252Fc%20d+e?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=provider%2F20260929%2Fregion%2Fs3%2Faws4_request&X-Amz-SignedHeaders=content-type%3Bhost&X-Amz-Signature=AbC%2B123%2Fxyz";

    for config in [aws_config_with_static_credentials(), compatible_config()] {
        assert!(is_custom_s3_config(&config));
        assert_eq!(
            transform_browser_url_for_config(
                input,
                &config,
                Some("http://localstack:4566"),
                Some("http://localhost:29806"),
            )
            .as_bytes(),
            input.as_bytes()
        );
        assert_eq!(
            transform_internal_url_for_config(input, &config, Some("http://localstack:4566"),)
                .as_bytes(),
            input.as_bytes()
        );
    }
}

#[test]
fn local_signed_url_transform_preserves_encoded_key_and_signature_bytes() {
    let input = "http://static-file-storage.localstack:4566/folder/a%2Fb%252Fc%20d+e?X-Amz-Credential=ANOTREAL%2Fscope&X-Amz-Signature=AbC%2B123%2Fxyz";
    let expected = "http://localhost:29806/static-file-storage/folder/a%2Fb%252Fc%20d+e?X-Amz-Credential=ANOTREAL%2Fscope&X-Amz-Signature=AbC%2B123%2Fxyz";

    let public = transform_local_url(input, Some("http://localhost:29806"));
    assert_eq!(public.as_bytes(), expected.as_bytes());
    assert_eq!(
        transform_internal_url(&public, "http://localstack:4566").as_bytes(),
        "http://localstack:4566/static-file-storage/folder/a%2Fb%252Fc%20d+e?X-Amz-Credential=ANOTREAL%2Fscope&X-Amz-Signature=AbC%2B123%2Fxyz".as_bytes()
    );
}

#[test]
fn test_transform_path_style_localstack() {
    let input = "http://localstack:4566/doc-storage/macro%7Cteo%40macro.com/doc/1?x-id=PutObject";
    let expected = "http://localhost:4566/doc-storage/macro%7Cteo%40macro.com/doc/1?x-id=PutObject";

    let result = transform_local_url(input, None);
    assert_eq!(result, expected);
}

#[test]
fn test_transform_path_style_localhost() {
    let input = "http://localhost:4566/doc-storage/macro%7Cteo%40macro.com/doc/1?x-id=PutObject";
    let expected = "http://localhost:4566/doc-storage/macro%7Cteo%40macro.com/doc/1?x-id=PutObject";

    let result = transform_local_url(input, None);
    assert_eq!(result, expected);
}

#[test]
fn test_transform_presigned_url_with_query_params_localstack() {
    let input = "http://static-file-storage.localstack:4566/file/a31e9af3-dd26-4531-b367-bfbbbac706cc?x-id=PutObject&X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=ANOTREAL%2F20260203%2Fus-east-1%2Fs3%2Faws4_request&X-Amz-Date=20260203T184319Z&X-Amz-Expires=120&X-Amz-SignedHeaders=content-type%3Bhost&X-Amz-Signature=deed6b123a18335b61567eaf8ddb7ea6e00bf264cfd80cb0f4031860235dc077";

    let expected = "http://localhost:4566/static-file-storage/file/a31e9af3-dd26-4531-b367-bfbbbac706cc?x-id=PutObject&X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=ANOTREAL%2F20260203%2Fus-east-1%2Fs3%2Faws4_request&X-Amz-Date=20260203T184319Z&X-Amz-Expires=120&X-Amz-SignedHeaders=content-type%3Bhost&X-Amz-Signature=deed6b123a18335b61567eaf8ddb7ea6e00bf264cfd80cb0f4031860235dc077";

    let result = transform_local_url(input, None);
    assert_eq!(result, expected);
}

#[test]
fn test_transform_presigned_url_with_query_params_localhost() {
    let input = "http://static-file-storage.localhost:4566/file/a31e9af3-dd26-4531-b367-bfbbbac706cc?x-id=PutObject&X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=ANOTREAL%2F20260203%2Fus-east-1%2Fs3%2Faws4_request&X-Amz-Date=20260203T184319Z&X-Amz-Expires=120&X-Amz-SignedHeaders=content-type%3Bhost&X-Amz-Signature=deed6b123a18335b61567eaf8ddb7ea6e00bf264cfd80cb0f4031860235dc077";

    let expected = "http://localhost:4566/static-file-storage/file/a31e9af3-dd26-4531-b367-bfbbbac706cc?x-id=PutObject&X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=ANOTREAL%2F20260203%2Fus-east-1%2Fs3%2Faws4_request&X-Amz-Date=20260203T184319Z&X-Amz-Expires=120&X-Amz-SignedHeaders=content-type%3Bhost&X-Amz-Signature=deed6b123a18335b61567eaf8ddb7ea6e00bf264cfd80cb0f4031860235dc077";

    let result = transform_local_url(input, None);
    assert_eq!(result, expected);
}

#[test]
fn test_transform_simple_url_localstack() {
    let input = "http://my-bucket.localstack:4566/some/path/to/file.txt";
    let expected = "http://localhost:4566/my-bucket/some/path/to/file.txt";

    let result = transform_local_url(input, None);
    assert_eq!(result, expected);
}

#[test]
fn test_transform_simple_url_localhost() {
    let input = "http://my-bucket.localhost:4566/some/path/to/file.txt";
    let expected = "http://localhost:4566/my-bucket/some/path/to/file.txt";

    let result = transform_local_url(input, None);
    assert_eq!(result, expected);
}

#[test]
fn test_transform_url_root_path() {
    let input = "http://bucket.localstack:4566/";
    let expected = "http://localhost:4566/bucket/";

    let result = transform_local_url(input, None);
    assert_eq!(result, expected);
}

#[test]
fn test_transform_url_no_path() {
    let input = "http://bucket.localhost:4566";
    let expected = "http://localhost:4566/bucket/";

    let result = transform_local_url(input, None);
    assert_eq!(result, expected);
}

#[test]
fn test_transform_url_with_simple_query() {
    let input = "http://test-bucket.localstack:4566/key?versionId=123";
    let expected = "http://localhost:4566/test-bucket/key?versionId=123";

    let result = transform_local_url(input, None);
    assert_eq!(result, expected);
}

#[test]
fn test_transform_url_with_simple_query_localhost() {
    let input = "http://test-bucket.localhost:4566/key?versionId=123";
    let expected = "http://localhost:4566/test-bucket/key?versionId=123";

    let result = transform_local_url(input, None);
    assert_eq!(result, expected);
}

#[test]
fn test_transform_url_default_port_localstack() {
    let input = "http://bucket.localstack/path/file.txt";
    let expected = "http://localhost:4566/bucket/path/file.txt";

    let result = transform_local_url(input, None);
    assert_eq!(result, expected);
}

#[test]
fn test_transform_url_default_port_localhost() {
    let input = "http://bucket.localhost/path/file.txt";
    let expected = "http://localhost:4566/bucket/path/file.txt";

    let result = transform_local_url(input, None);
    assert_eq!(result, expected);
}

#[test]
fn test_internal_fetch_rewrites_localhost_to_localstack() {
    let input = "http://localhost:4566/doc-storage/macro%7Cteo%40macro.com/doc/1";
    let expected = "http://localstack:4566/doc-storage/macro%7Cteo%40macro.com/doc/1";

    let result = transform_internal_url(input, "http://localstack:4566");
    assert_eq!(result, expected);
}

#[test]
fn test_internal_fetch_preserves_query_params() {
    let input = "http://localhost:4566/doc-storage/key?versionId=123";
    let expected = "http://localstack:4566/doc-storage/key?versionId=123";

    let result = transform_internal_url(input, "http://localstack:4566");
    assert_eq!(result, expected);
}

#[test]
fn test_internal_fetch_localstack_is_idempotent() {
    let input = "http://localstack:4566/doc-storage/key";
    let expected = "http://localstack:4566/doc-storage/key";

    let result = transform_internal_url(input, "http://localstack:4566");
    assert_eq!(result, expected);
}

#[test]
fn test_internal_fetch_leaves_remote_url_untouched() {
    let input = "https://d123.cloudfront.net/doc-storage/key?Signature=abc";
    let expected = "https://d123.cloudfront.net/doc-storage/key?Signature=abc";

    let result = transform_internal_url(input, "http://localstack:4566");
    assert_eq!(result, expected);
}

#[test]
fn named_instance_uses_public_port_and_preserves_signature_and_encoded_key() {
    for input in [
        "http://localstack:4566/static-file-storage/file/pasted%20image.png?X-Amz-Signature=test&x-id=PutObject",
        "http://static-file-storage.localstack:4566/file/pasted%20image.png?X-Amz-Signature=test&x-id=PutObject",
    ] {
        let public = transform_local_url(input, Some("http://localhost:29806"));
        assert_eq!(
            public,
            "http://localhost:29806/static-file-storage/file/pasted%20image.png?X-Amz-Signature=test&x-id=PutObject"
        );
        assert_eq!(
            transform_internal_url(&public, "http://localstack:4566"),
            "http://localstack:4566/static-file-storage/file/pasted%20image.png?X-Amz-Signature=test&x-id=PutObject"
        );
    }
}

#[test]
fn host_process_keeps_its_configured_localstack_endpoint_for_internal_fetch() {
    let input = "http://localhost:29806/doc-storage/key";
    assert_eq!(
        transform_internal_url(input, "http://localhost:29806"),
        input
    );
}
