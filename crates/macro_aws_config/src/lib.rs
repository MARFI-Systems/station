#![deny(missing_docs)]

//! This crate creates a standard way to make AWS configs.

use std::fmt;

pub use aws_config::SdkConfig;
use macro_env_var::maybe_env_var;
use thiserror::Error;

maybe_env_var! {
    #[derive(Clone)]
    pub struct LocalAwsUrl;
}

maybe_env_var! {
    /// Browser-facing LocalStack origin for a local stack with mapped ports.
    #[derive(Clone)]
    pub struct LocalAwsPublicUrl;
}

const OBJECT_STORAGE_MODE: &str = "OBJECT_STORAGE_MODE";
const OBJECT_STORAGE_REGION: &str = "OBJECT_STORAGE_REGION";
const OBJECT_STORAGE_ENDPOINT_URL: &str = "OBJECT_STORAGE_ENDPOINT_URL";
const OBJECT_STORAGE_ACCESS_KEY_ID: &str = "OBJECT_STORAGE_ACCESS_KEY_ID";
const OBJECT_STORAGE_SECRET_ACCESS_KEY: &str = "OBJECT_STORAGE_SECRET_ACCESS_KEY";
const OBJECT_STORAGE_SESSION_TOKEN: &str = "OBJECT_STORAGE_SESSION_TOKEN";
const AWS_ACCESS_KEY_ID: &str = "AWS_ACCESS_KEY_ID";
const AWS_SECRET_ACCESS_KEY: &str = "AWS_SECRET_ACCESS_KEY";
const AWS_SESSION_TOKEN: &str = "AWS_SESSION_TOKEN";

/// An invalid explicit object-storage configuration.
///
/// Errors identify variable names and configuration rules, but never include
/// credential values.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ObjectStorageConfigError {
    /// A configuration variable could not be read as Unicode.
    #[error("could not read configuration variable {variable}")]
    UnreadableVariable {
        /// The invalid variable name.
        variable: &'static str,
    },
    /// Provider-specific settings were supplied without an explicit mode.
    #[error(
        "{variable} requires an explicit OBJECT_STORAGE_MODE of aws or s3-compatible"
    )]
    ModeRequired {
        /// The variable that requires an explicit mode.
        variable: &'static str,
    },
    /// The selected mode is not supported.
    #[error("OBJECT_STORAGE_MODE must be legacy, aws, or s3-compatible")]
    UnsupportedMode,
    /// A required setting is missing.
    #[error("{variable} is required when OBJECT_STORAGE_MODE={mode}")]
    MissingVariable {
        /// The missing variable name.
        variable: &'static str,
        /// The selected object-storage mode.
        mode: &'static str,
    },
    /// A setting is not accepted in the selected mode.
    #[error("{variable} is not allowed when OBJECT_STORAGE_MODE={mode}")]
    UnexpectedVariable {
        /// The unexpected variable name.
        variable: &'static str,
        /// The selected object-storage mode.
        mode: &'static str,
    },
    /// Static credentials were only partially configured.
    #[error(
        "OBJECT_STORAGE_ACCESS_KEY_ID and OBJECT_STORAGE_SECRET_ACCESS_KEY must be set together"
    )]
    PartialCredentials,
    /// The local AWS environment exposes static credentials to a real AWS S3 client.
    #[error(
        "ambient AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, or AWS_SESSION_TOKEN values are not allowed for OBJECT_STORAGE_MODE=aws while LOCAL_AWS_URL is set; remove the ambient development credentials and use an IAM role, or set dedicated OBJECT_STORAGE credentials"
    )]
    AmbientAwsCredentialsWithLocalEndpoint,
    /// A configured value failed validation.
    #[error("{variable} is invalid: {reason}")]
    InvalidValue {
        /// The invalid variable name.
        variable: &'static str,
        /// A redacted validation reason.
        reason: &'static str,
    },
}

#[derive(Clone)]
struct RawObjectStorageConfig {
    mode: Option<String>,
    region: Option<String>,
    endpoint_url: Option<String>,
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
    session_token: Option<String>,
}

impl RawObjectStorageConfig {
    fn from_env() -> Result<Self, ObjectStorageConfigError> {
        Ok(Self {
            mode: read_optional(OBJECT_STORAGE_MODE)?,
            region: read_optional(OBJECT_STORAGE_REGION)?,
            endpoint_url: read_optional(OBJECT_STORAGE_ENDPOINT_URL)?,
            access_key_id: read_optional(OBJECT_STORAGE_ACCESS_KEY_ID)?,
            secret_access_key: read_optional(OBJECT_STORAGE_SECRET_ACCESS_KEY)?,
            session_token: read_optional(OBJECT_STORAGE_SESSION_TOKEN)?,
        })
    }

    fn first_provider_setting(&self) -> Option<&'static str> {
        [
            (OBJECT_STORAGE_REGION, self.region.is_some()),
            (OBJECT_STORAGE_ENDPOINT_URL, self.endpoint_url.is_some()),
            (
                OBJECT_STORAGE_ACCESS_KEY_ID,
                self.access_key_id.is_some(),
            ),
            (
                OBJECT_STORAGE_SECRET_ACCESS_KEY,
                self.secret_access_key.is_some(),
            ),
            (OBJECT_STORAGE_SESSION_TOKEN, self.session_token.is_some()),
        ]
        .into_iter()
        .find_map(|(name, is_set)| is_set.then_some(name))
    }
}

#[derive(Clone, PartialEq, Eq)]
enum ObjectStorageCredentials {
    DefaultChain,
    Static {
        access_key_id: String,
        secret_access_key: String,
        session_token: Option<String>,
    },
}

impl fmt::Debug for ObjectStorageCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DefaultChain => formatter.write_str("DefaultChain"),
            Self::Static { .. } => formatter.write_str("Static(<redacted>)"),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
enum ObjectStorageConfig {
    Legacy,
    Aws {
        region: String,
        credentials: ObjectStorageCredentials,
    },
    S3Compatible {
        region: String,
        endpoint_url: String,
        credentials: ObjectStorageCredentials,
    },
}

#[derive(Clone, Copy, Default)]
struct AmbientAwsCredentialFields {
    access_key_id: bool,
    secret_access_key: bool,
    session_token: bool,
}

impl AmbientAwsCredentialFields {
    fn from_env() -> Result<Self, ObjectStorageConfigError> {
        Ok(Self {
            access_key_id: read_optional(AWS_ACCESS_KEY_ID)?.is_some(),
            secret_access_key: read_optional(AWS_SECRET_ACCESS_KEY)?.is_some(),
            session_token: read_optional(AWS_SESSION_TOKEN)?.is_some(),
        })
    }

    fn any_set(self) -> bool {
        self.access_key_id || self.secret_access_key || self.session_token
    }
}

impl fmt::Debug for ObjectStorageConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Legacy => formatter.debug_struct("ObjectStorageConfig::Legacy").finish(),
            Self::Aws {
                region,
                credentials,
            } => formatter
                .debug_struct("ObjectStorageConfig::Aws")
                .field("region", region)
                .field("credentials", credentials)
                .finish(),
            Self::S3Compatible {
                region,
                credentials,
                ..
            } => formatter
                .debug_struct("ObjectStorageConfig::S3Compatible")
                .field("region", region)
                .field("endpoint_url", &"<configured>")
                .field("credentials", credentials)
                .finish(),
        }
    }
}

impl TryFrom<RawObjectStorageConfig> for ObjectStorageConfig {
    type Error = ObjectStorageConfigError;

    fn try_from(raw: RawObjectStorageConfig) -> Result<Self, Self::Error> {
        let Some(mode) = raw.mode.as_deref() else {
            if let Some(variable) = raw.first_provider_setting() {
                return Err(ObjectStorageConfigError::ModeRequired { variable });
            }
            return Ok(Self::Legacy);
        };

        match mode {
            "legacy" => {
                if let Some(variable) = raw.first_provider_setting() {
                    return Err(ObjectStorageConfigError::UnexpectedVariable {
                        variable,
                        mode: "legacy",
                    });
                }
                Ok(Self::Legacy)
            }
            "aws" => {
                if raw.endpoint_url.is_some() {
                    return Err(ObjectStorageConfigError::UnexpectedVariable {
                        variable: OBJECT_STORAGE_ENDPOINT_URL,
                        mode: "aws",
                    });
                }
                Ok(Self::Aws {
                    region: validate_region(raw.region, "aws")?,
                    credentials: validate_credentials(
                        raw.access_key_id,
                        raw.secret_access_key,
                        raw.session_token,
                        false,
                        "aws",
                    )?,
                })
            }
            "s3-compatible" => Ok(Self::S3Compatible {
                region: validate_region(raw.region, "s3-compatible")?,
                endpoint_url: validate_tls_endpoint(raw.endpoint_url)?,
                credentials: validate_credentials(
                    raw.access_key_id,
                    raw.secret_access_key,
                    raw.session_token,
                    true,
                    "s3-compatible",
                )?,
            }),
            _ => Err(ObjectStorageConfigError::UnsupportedMode),
        }
    }
}

fn read_optional(variable: &'static str) -> Result<Option<String>, ObjectStorageConfigError> {
    macro_env_var::optional_read_env_var(variable)
        .map_err(|_| ObjectStorageConfigError::UnreadableVariable { variable })
}

fn validate_region(
    region: Option<String>,
    mode: &'static str,
) -> Result<String, ObjectStorageConfigError> {
    let region = region.ok_or(ObjectStorageConfigError::MissingVariable {
        variable: OBJECT_STORAGE_REGION,
        mode,
    })?;

    if region.is_empty()
        || region.trim() != region
        || !region
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(ObjectStorageConfigError::InvalidValue {
            variable: OBJECT_STORAGE_REGION,
            reason: "expected a non-empty region containing only letters, numbers, and hyphens",
        });
    }

    Ok(region)
}

fn validate_credentials(
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
    session_token: Option<String>,
    required: bool,
    mode: &'static str,
) -> Result<ObjectStorageCredentials, ObjectStorageConfigError> {
    match (access_key_id, secret_access_key, session_token) {
        (None, None, None) if required => Err(ObjectStorageConfigError::MissingVariable {
            variable: OBJECT_STORAGE_ACCESS_KEY_ID,
            mode,
        }),
        (None, None, None) => Ok(ObjectStorageCredentials::DefaultChain),
        (Some(access_key_id), Some(secret_access_key), session_token) => {
            if access_key_id.is_empty() {
                return Err(ObjectStorageConfigError::InvalidValue {
                    variable: OBJECT_STORAGE_ACCESS_KEY_ID,
                    reason: "must not be empty",
                });
            }
            if secret_access_key.is_empty() {
                return Err(ObjectStorageConfigError::InvalidValue {
                    variable: OBJECT_STORAGE_SECRET_ACCESS_KEY,
                    reason: "must not be empty",
                });
            }
            if session_token.as_deref() == Some("") {
                return Err(ObjectStorageConfigError::InvalidValue {
                    variable: OBJECT_STORAGE_SESSION_TOKEN,
                    reason: "must not be empty when set",
                });
            }

            Ok(ObjectStorageCredentials::Static {
                access_key_id,
                secret_access_key,
                session_token,
            })
        }
        _ => Err(ObjectStorageConfigError::PartialCredentials),
    }
}

fn validate_tls_endpoint(
    endpoint_url: Option<String>,
) -> Result<String, ObjectStorageConfigError> {
    let endpoint_url = endpoint_url.ok_or(ObjectStorageConfigError::MissingVariable {
        variable: OBJECT_STORAGE_ENDPOINT_URL,
        mode: "s3-compatible",
    })?;

    if endpoint_url.trim() != endpoint_url {
        return Err(ObjectStorageConfigError::InvalidValue {
            variable: OBJECT_STORAGE_ENDPOINT_URL,
            reason: "must not contain surrounding whitespace",
        });
    }

    let parsed = url::Url::parse(&endpoint_url).map_err(|_| {
        ObjectStorageConfigError::InvalidValue {
            variable: OBJECT_STORAGE_ENDPOINT_URL,
            reason: "must be an absolute HTTPS URL",
        }
    })?;

    if parsed.scheme() != "https" || parsed.host_str().is_none() {
        return Err(ObjectStorageConfigError::InvalidValue {
            variable: OBJECT_STORAGE_ENDPOINT_URL,
            reason: "must be an absolute HTTPS URL",
        });
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(ObjectStorageConfigError::InvalidValue {
            variable: OBJECT_STORAGE_ENDPOINT_URL,
            reason: "must not contain user information",
        });
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(ObjectStorageConfigError::InvalidValue {
            variable: OBJECT_STORAGE_ENDPOINT_URL,
            reason: "must not contain a query or fragment",
        });
    }
    if parsed.path() != "/" && !parsed.path().is_empty() {
        return Err(ObjectStorageConfigError::InvalidValue {
            variable: OBJECT_STORAGE_ENDPOINT_URL,
            reason: "must not contain a path",
        });
    }

    Ok(endpoint_url.trim_end_matches('/').to_owned())
}

fn validate_default_chain_isolation(
    config: &ObjectStorageConfig,
    local_aws_url_set: bool,
    ambient_credentials: AmbientAwsCredentialFields,
) -> Result<(), ObjectStorageConfigError> {
    let uses_aws_default_chain = matches!(
        config,
        ObjectStorageConfig::Aws {
            credentials: ObjectStorageCredentials::DefaultChain,
            ..
        }
    );

    if uses_aws_default_chain && local_aws_url_set && ambient_credentials.any_set() {
        return Err(ObjectStorageConfigError::AmbientAwsCredentialsWithLocalEndpoint);
    }

    Ok(())
}

fn object_storage_config() -> Result<ObjectStorageConfig, ObjectStorageConfigError> {
    let config: ObjectStorageConfig = RawObjectStorageConfig::from_env()?.try_into()?;

    if matches!(
        &config,
        ObjectStorageConfig::Aws {
            credentials: ObjectStorageCredentials::DefaultChain,
            ..
        }
    ) && LocalAwsUrl::new().is_some()
    {
        validate_default_chain_isolation(
            &config,
            true,
            AmbientAwsCredentialFields::from_env()?,
        )?;
    }

    Ok(config)
}

fn is_custom_s3_config(config: &ObjectStorageConfig) -> bool {
    matches!(
        config,
        ObjectStorageConfig::Aws { .. } | ObjectStorageConfig::S3Compatible { .. }
    )
}

fn object_storage_config_or_panic() -> ObjectStorageConfig {
    object_storage_config()
        .unwrap_or_else(|error| panic!("invalid object-storage configuration: {error}"))
}

/// Creates an S3 client.
///
/// This preserves legacy behavior unless `OBJECT_STORAGE_MODE` explicitly
/// selects a remote provider mode. Invalid explicit configuration terminates
/// initialization instead of falling back to the shared AWS configuration.
#[cfg(feature = "s3")]
pub async fn s3_client() -> aws_sdk_s3::Client {
    try_s3_client()
        .await
        .unwrap_or_else(|error| panic!("invalid object-storage configuration: {error}"))
}

/// Tries to create an S3 client from the explicit object-storage configuration.
#[cfg(feature = "s3")]
pub async fn try_s3_client() -> Result<aws_sdk_s3::Client, ObjectStorageConfigError> {
    let config = object_storage_config()?;
    let s3_config = match config {
        ObjectStorageConfig::Legacy => {
            aws_sdk_s3::config::Builder::from(&get_macro_aws_config().await)
                .force_path_style(is_local_aws())
                .build()
        }
        remote => remote_s3_config(remote).await,
    };
    Ok(aws_sdk_s3::Client::from_conf(s3_config))
}

#[cfg(feature = "s3")]
async fn remote_s3_config(config: ObjectStorageConfig) -> aws_sdk_s3::Config {
    use aws_sdk_s3::config::Region;

    let (region, uses_default_credentials) = match &config {
        ObjectStorageConfig::Aws {
            region,
            credentials,
        }
        | ObjectStorageConfig::S3Compatible {
            region,
            credentials,
            ..
        } => (
            region.clone(),
            matches!(credentials, ObjectStorageCredentials::DefaultChain),
        ),
        ObjectStorageConfig::Legacy => unreachable!("legacy uses the shared AWS configuration"),
    };

    let loader = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(Region::new(region));
    let shared_config = if uses_default_credentials {
        loader.load().await
    } else {
        loader.no_credentials().load().await
    };

    apply_remote_s3_overrides(aws_sdk_s3::config::Builder::from(&shared_config), config).build()
}

#[cfg(feature = "s3")]
fn apply_remote_s3_overrides(
    mut builder: aws_sdk_s3::config::Builder,
    config: ObjectStorageConfig,
) -> aws_sdk_s3::config::Builder {
    use aws_sdk_s3::config::{Credentials, Region, SharedCredentialsProvider};

    let (region, endpoint_url, force_path_style, credentials) = match config {
        ObjectStorageConfig::Aws {
            region,
            credentials,
        } => (region, None, false, credentials),
        ObjectStorageConfig::S3Compatible {
            region,
            endpoint_url,
            credentials,
        } => (region, Some(endpoint_url), true, credentials),
        ObjectStorageConfig::Legacy => unreachable!("legacy uses the shared AWS configuration"),
    };

    builder.set_region(Some(Region::new(region)));
    // `Builder::from(&SdkConfig)` materializes both generic `AWS_ENDPOINT_URL`
    // and S3-specific `AWS_ENDPOINT_URL_S3` settings into this service-level
    // endpoint field. Always overwrite that inherited field: real AWS uses its
    // regional endpoint and compatible providers use only the validated URL.
    builder.set_endpoint_url(endpoint_url);
    builder.set_force_path_style(Some(force_path_style));

    if let ObjectStorageCredentials::Static {
        access_key_id,
        secret_access_key,
        session_token,
    } = credentials
    {
        builder.set_credentials_provider(Some(SharedCredentialsProvider::new(Credentials::new(
            access_key_id,
            secret_access_key,
            session_token,
            None,
            "object-storage",
        ))));
    }

    builder
}

/// Creates an SQS client.
#[cfg(feature = "sqs")]
pub async fn sqs_client() -> aws_sdk_sqs::Client {
    aws_sdk_sqs::Client::new(&get_macro_aws_config().await)
}

/// Creates a shared AWS SDK configuration.
///
/// If `LOCAL_AWS_URL` is present, this creates a local AWS configuration with
/// test credentials. Otherwise it loads the normal AWS provider chain. This
/// legacy shared behavior remains independent from explicit S3 configuration.
pub async fn get_macro_aws_config() -> aws_config::SdkConfig {
    if let Some(local_aws_url) = LocalAwsUrl::new() {
        local_aws_config(local_aws_url.as_ref()).await
    } else {
        aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region("us-east-1")
            .load()
            .await
    }
}

/// Creates an AWS SDK config pointed at a LocalStack endpoint.
pub async fn local_aws_config(local_aws_url: &str) -> aws_config::SdkConfig {
    aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region("us-east-1")
        .test_credentials()
        .endpoint_url(local_aws_url)
        .load()
        .await
}

/// Returns whether the shared AWS configuration is local.
///
/// This intentionally retains its original `LOCAL_AWS_URL` semantics for SQS,
/// Secrets Manager, DynamoDB, and existing callers.
pub fn is_local_aws() -> bool {
    LocalAwsUrl::new().is_some()
}

/// Returns whether S3 itself uses the legacy local AWS endpoint.
///
/// Invalid explicit object-storage configuration fails closed.
pub fn is_local_s3() -> bool {
    matches!(object_storage_config_or_panic(), ObjectStorageConfig::Legacy) && is_local_aws()
}

/// Returns whether an explicit valid remote S3 configuration is selected.
///
/// This returns `false` for the legacy default and fails closed if any explicit
/// object-storage configuration is invalid.
pub fn is_custom_s3_configured() -> bool {
    is_custom_s3_config(&object_storage_config_or_panic())
}

/// Internal method to transform the local AWS URL.
fn transform_local_url(url: &str, public_url: Option<&str>) -> String {
    // NOTE: it is ok to use expect as this is only run locally
    let parsed = url::Url::parse(url).expect("valid url");
    let host = parsed.host_str().unwrap();
    let port = parsed.port().unwrap_or(4566);
    let path = parsed.path();
    let query = parsed.query().map(|q| format!("?{q}")).unwrap_or_default();

    let origin = public_url
        .map(|url| url.trim_end_matches('/').to_owned())
        .unwrap_or_else(|| format!("http://localhost:{port}"));

    // Path-style LocalStack URLs generated inside Docker use `localstack` as
    // the host, which the browser on the host machine cannot resolve. Keep the
    // existing path (`/{bucket}/{key}`) and use the browser-facing origin.
    if host == "localstack" || host == "localhost" {
        return format!("{origin}{path}{query}");
    }

    // hostname should be in the form {asset}.localstack or {asset}.localhost
    let asset = host
        .strip_suffix(".localstack")
        .or_else(|| host.strip_suffix(".localhost"))
        .unwrap();

    format!("{origin}/{asset}{path}{query}")
}

fn transform_browser_url_for_config(
    url: &str,
    config: &ObjectStorageConfig,
    local_aws_url: Option<&str>,
    public_url: Option<&str>,
) -> String {
    if matches!(config, ObjectStorageConfig::Legacy) && local_aws_url.is_some() {
        transform_local_url(url, public_url)
    } else {
        url.to_owned()
    }
}

/// Transforms a LocalStack URL into one that will work within the app.
///
/// Explicit remote object-storage modes are always returned byte-for-byte,
/// even when `LOCAL_AWS_URL` remains enabled for other AWS services.
pub fn transform_aws_url(url: &str) -> String {
    let config = object_storage_config_or_panic();
    let local_aws_url = LocalAwsUrl::new();
    let public_url = LocalAwsPublicUrl::new();
    transform_browser_url_for_config(
        url,
        &config,
        local_aws_url.as_ref().map(|value| value.as_ref()),
        public_url.as_ref().map(|value| value.as_ref()),
    )
}

/// Internal method to transform a browser-facing local URL into one reachable
/// from inside the Docker network.
fn transform_internal_url(url: &str, local_aws_url: &str) -> String {
    // NOTE: it is ok to use expect as this is only run locally
    let parsed = url::Url::parse(url).expect("valid url");
    let host = parsed.host_str().unwrap();
    let path = parsed.path();
    let query = parsed.query().map(|q| format!("?{q}")).unwrap_or_default();

    // Browser-facing local URLs use `localhost`, which inside a container
    // resolves to the container itself. Use the SDK endpoint, including its
    // internal port, so service-to-service fetches reach LocalStack. Leave any
    // other host untouched.
    if host == "localhost" || host == "localstack" {
        return format!("{}{path}{query}", local_aws_url.trim_end_matches('/'));
    }

    url.to_string()
}

fn transform_internal_url_for_config(
    url: &str,
    config: &ObjectStorageConfig,
    local_aws_url: Option<&str>,
) -> String {
    if matches!(config, ObjectStorageConfig::Legacy) {
        if let Some(local_aws_url) = local_aws_url {
            return transform_internal_url(url, local_aws_url);
        }
    }
    url.to_owned()
}

/// Transforms a browser-facing local URL into one reachable from inside the
/// app's own containers when fetching an object server-side.
///
/// This remains the inverse of [`transform_aws_url`] in legacy LocalStack mode.
/// Explicit remote provider URLs are never rewritten.
pub fn transform_aws_url_for_internal_fetch(url: &str) -> String {
    let config = object_storage_config_or_panic();
    let local_aws_url = LocalAwsUrl::new();
    transform_internal_url_for_config(
        url,
        &config,
        local_aws_url.as_ref().map(|value| value.as_ref()),
    )
}

#[cfg(test)]
mod test;
