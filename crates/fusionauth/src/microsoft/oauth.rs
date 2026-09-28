use std::time::Duration;

use anyhow::Context;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use reqwest::Url;

use crate::{
    Result, UnauthedClient,
    error::{FusionAuthClientError, GenericErrorResponse},
};

pub(crate) const MICROSOFT_LOGIN_BASE_URL: &str = "https://login.microsoftonline.com/";
const MICROSOFT_IDENTITY_SCOPES: &str = "openid email offline_access profile";
const MICROSOFT_MAILBOX_SCOPES: &str = "openid email offline_access profile Mail.Read";

#[derive(serde::Serialize)]
struct MicrosoftTokenExchangeRequest<'a> {
    client_id: &'a str,
    client_secret: &'a str,
    code: &'a str,
    code_verifier: Option<&'a str>,
    grant_type: &'static str,
    redirect_uri: &'a str,
}

#[derive(serde::Serialize)]
struct MicrosoftRefreshRequest<'a> {
    client_id: &'a str,
    client_secret: &'a str,
    refresh_token: &'a str,
    grant_type: &'static str,
    scope: &'static str,
}

#[derive(serde::Deserialize)]
struct MicrosoftTokenExchangePayload {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<u64>,
    token_type: Option<String>,
    scope: Option<String>,
}

/// Tokens returned by a Microsoft authorization-code exchange.
pub struct MicrosoftExchangeTokenResponse {
    /// The refresh token used to request future Microsoft Graph access tokens.
    pub refresh_token: String,
    /// The ID token containing the linked Microsoft identity.
    pub id_token: String,
}

/// A delegated Microsoft Graph access token and optional rotated refresh token.
pub struct MicrosoftMailboxAccessTokenResponse {
    /// Short-lived delegated Graph token.
    pub access_token: String,
    /// Rotated refresh token, when Microsoft issued one.
    pub refresh_token: Option<String>,
    /// Token lifetime in seconds.
    pub expires_in: u64,
    /// Provider token type.
    pub token_type: String,
}

#[derive(serde::Deserialize)]
struct MicrosoftIdTokenClaims {
    tid: String,
    oid: String,
    sub: String,
    email: Option<String>,
    preferred_username: Option<String>,
}

#[derive(serde::Deserialize)]
struct MicrosoftOpenIdConfiguration {
    issuer: String,
    jwks_uri: String,
}

#[derive(serde::Deserialize)]
struct MicrosoftJsonWebKeySet {
    keys: Vec<MicrosoftJsonWebKey>,
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct MicrosoftJsonWebKey {
    pub(crate) kid: String,
    pub(crate) kty: String,
    pub(crate) n: String,
    pub(crate) e: String,
}

/// The tenant's OIDC issuer and current signing keys, resolved through Microsoft OIDC discovery.
#[derive(Debug)]
pub(crate) struct MicrosoftSigningKeys {
    pub(crate) issuer: String,
    pub(crate) keys: Vec<MicrosoftJsonWebKey>,
}

/// Validated identity details extracted from a Microsoft ID token.
#[derive(Debug, Eq, PartialEq)]
pub struct MicrosoftUserInfo {
    /// The OIDC subject used only for identity-provider linking.
    pub sub: String,
    /// Verified Entra tenant identifier.
    pub tenant_id: String,
    /// Verified stable Entra object identifier for the mailbox account.
    pub object_id: String,
    /// The email claim, or `preferred_username` when the email claim is absent.
    pub email: String,
}

pub(crate) fn construct_authorize_url<T>(
    client_id: &str,
    tenant_id: &str,
    redirect_uri: &str,
    state: &T,
) -> anyhow::Result<String>
where
    T: serde::Serialize + ?Sized,
{
    let mut url = endpoint_url(tenant_id, "authorize")?;
    let serialized_state =
        serde_json::to_string(state).context("failed to serialize Microsoft OAuth state")?;

    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", MICROSOFT_IDENTITY_SCOPES)
        .append_pair("state", &serialized_state)
        .append_pair("prompt", "select_account");

    Ok(url.to_string())
}

pub(crate) async fn exchange_code_for_tokens(
    client: &UnauthedClient,
    client_id: &str,
    client_secret: &str,
    tenant_id: &str,
    redirect_uri: &str,
    code: &str,
) -> Result<MicrosoftExchangeTokenResponse> {
    let token_endpoint = endpoint_url(tenant_id, "token").map_err(FusionAuthClientError::from)?;
    let request = MicrosoftTokenExchangeRequest {
        client_id,
        client_secret,
        code,
        code_verifier: None,
        grant_type: "authorization_code",
        redirect_uri,
    };

    let response = client
        .client()
        .post(token_endpoint)
        .form(&request)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|error| {
            tracing::error!(error=?error, "failed to send Microsoft token request");
            generic_error(error.to_string())
        })?;
    let status = response.status();

    if !status.is_success() {
        let error_body = response.text().await.map_err(|error| {
            tracing::error!(error=?error, "failed to read Microsoft token error response");
            generic_error(error.to_string())
        })?;
        tracing::error!(?status, body=?error_body, "Microsoft token exchange failed");
        return Err(generic_error(format!(
            "Microsoft token exchange failed with status {status}: {error_body}"
        )));
    }

    let payload = response
        .json::<MicrosoftTokenExchangePayload>()
        .await
        .map_err(|error| {
            tracing::error!(error=?error, "failed to parse Microsoft token response");
            generic_error(error.to_string())
        })?;

    let refresh_token = payload
        .refresh_token
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| generic_error("Microsoft token response did not include a refresh token"))?;
    let id_token = payload
        .id_token
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| generic_error("Microsoft token response did not include an ID token"))?;

    Ok(MicrosoftExchangeTokenResponse {
        refresh_token,
        id_token,
    })
}

pub(crate) fn construct_mailbox_authorize_url<T>(
    client_id: &str,
    tenant_id: &str,
    redirect_uri: &str,
    state: &T,
    code_challenge: &str,
) -> anyhow::Result<String>
where
    T: serde::Serialize + ?Sized,
{
    let mut url = endpoint_url(tenant_id, "authorize")?;
    let serialized_state = serde_json::to_string(state)
        .context("failed to serialize Microsoft mailbox OAuth state")?;
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", MICROSOFT_MAILBOX_SCOPES)
        .append_pair("state", &serialized_state)
        .append_pair("code_challenge", code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("prompt", "select_account");
    Ok(url.to_string())
}

pub(crate) async fn exchange_mailbox_code_for_tokens(
    client: &UnauthedClient,
    client_id: &str,
    client_secret: &str,
    tenant_id: &str,
    redirect_uri: &str,
    code: &str,
    code_verifier: &str,
) -> Result<(
    MicrosoftExchangeTokenResponse,
    MicrosoftMailboxAccessTokenResponse,
)> {
    let token_endpoint = endpoint_url(tenant_id, "token").map_err(FusionAuthClientError::from)?;
    let response = client
        .client()
        .post(token_endpoint)
        .form(&MicrosoftTokenExchangeRequest {
            client_id,
            client_secret,
            code,
            code_verifier: Some(code_verifier),
            grant_type: "authorization_code",
            redirect_uri,
        })
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|error| generic_error(error.to_string()))?;
    parse_mailbox_token_response(response, true)
        .await
        .map(|(access, id_token)| {
            (
                MicrosoftExchangeTokenResponse {
                    refresh_token: access.refresh_token.clone().expect("required by parser"),
                    id_token: id_token.expect("required by parser"),
                },
                access,
            )
        })
}

pub(crate) async fn refresh_mailbox_access_token(
    client: &UnauthedClient,
    client_id: &str,
    client_secret: &str,
    tenant_id: &str,
    refresh_token: &str,
) -> Result<MicrosoftMailboxAccessTokenResponse> {
    let token_endpoint = endpoint_url(tenant_id, "token").map_err(FusionAuthClientError::from)?;
    let response = client
        .client()
        .post(token_endpoint)
        .form(&MicrosoftRefreshRequest {
            client_id,
            client_secret,
            refresh_token,
            grant_type: "refresh_token",
            scope: MICROSOFT_MAILBOX_SCOPES,
        })
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|error| generic_error(error.to_string()))?;
    parse_mailbox_token_response(response, false)
        .await
        .map(|(access, _)| access)
}

async fn parse_mailbox_token_response(
    response: reqwest::Response,
    require_identity: bool,
) -> Result<(MicrosoftMailboxAccessTokenResponse, Option<String>)> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        if status == reqwest::StatusCode::BAD_REQUEST && body.contains("invalid_grant") {
            return Err(FusionAuthClientError::InvalidGrant);
        }
        return Err(generic_error(format!(
            "Microsoft token request failed with status {status}"
        )));
    }
    let payload = response
        .json::<MicrosoftTokenExchangePayload>()
        .await
        .map_err(|error| generic_error(error.to_string()))?;
    validate_mailbox_scopes(payload.scope.as_deref().unwrap_or_default())?;
    let access_token = payload
        .access_token
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| generic_error("Microsoft token response did not include an access token"))?;
    let id_token = payload.id_token.filter(|value| !value.trim().is_empty());
    if require_identity && id_token.is_none() {
        return Err(generic_error(
            "Microsoft token response did not include an ID token",
        ));
    }
    if require_identity
        && payload
            .refresh_token
            .as_ref()
            .is_none_or(|value| value.trim().is_empty())
    {
        return Err(generic_error(
            "Microsoft token response did not include a refresh token",
        ));
    }
    Ok((
        MicrosoftMailboxAccessTokenResponse {
            access_token,
            refresh_token: payload
                .refresh_token
                .filter(|value| !value.trim().is_empty()),
            expires_in: payload.expires_in.unwrap_or(0),
            token_type: payload.token_type.unwrap_or_else(|| "Bearer".into()),
        },
        id_token,
    ))
}

pub(crate) fn validate_mailbox_scopes(scopes: &str) -> Result<()> {
    let allowed = ["openid", "email", "offline_access", "profile", "Mail.Read"];
    let values: Vec<_> = scopes.split_whitespace().collect();
    if !values
        .iter()
        .any(|scope| scope.eq_ignore_ascii_case("Mail.Read"))
        || values.iter().any(|scope| {
            !allowed
                .iter()
                .any(|allowed| scope.eq_ignore_ascii_case(allowed))
        })
    {
        return Err(generic_error(
            "Microsoft mailbox token contains disallowed or missing scopes",
        ));
    }
    Ok(())
}

pub(crate) async fn fetch_microsoft_signing_keys(
    client: &UnauthedClient,
    login_base_url: &str,
    tenant_id: &str,
) -> anyhow::Result<MicrosoftSigningKeys> {
    let discovery_url = discovery_url(login_base_url, tenant_id)?;
    let configuration: MicrosoftOpenIdConfiguration =
        fetch_json(client, discovery_url, "OpenID configuration").await?;

    let jwks_uri = Url::parse(&configuration.jwks_uri)
        .context("Microsoft OpenID configuration contains an invalid JWKS URI")?;
    let jwks: MicrosoftJsonWebKeySet = fetch_json(client, jwks_uri, "JWKS").await?;
    if jwks.keys.is_empty() {
        anyhow::bail!("Microsoft JWKS response did not contain any signing keys");
    }

    Ok(MicrosoftSigningKeys {
        issuer: configuration.issuer,
        keys: jwks.keys,
    })
}

async fn fetch_json<T: serde::de::DeserializeOwned>(
    client: &UnauthedClient,
    url: Url,
    resource: &str,
) -> anyhow::Result<T> {
    let response = client
        .client()
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .with_context(|| format!("failed to fetch Microsoft {resource}"))?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("Microsoft {resource} request failed with status {status}");
    }

    response
        .json::<T>()
        .await
        .with_context(|| format!("failed to parse Microsoft {resource} response"))
}

pub(crate) fn decode_microsoft_id_token(
    id_token: &str,
    signing_keys: &MicrosoftSigningKeys,
    expected_audience: &str,
    expected_tenant_id: &str,
) -> anyhow::Result<MicrosoftUserInfo> {
    let header = jsonwebtoken::decode_header(id_token)
        .context("failed to decode Microsoft ID-token header")?;
    if header.alg != Algorithm::RS256 {
        anyhow::bail!("Microsoft ID token is not signed with an approved algorithm");
    }
    let kid = header
        .kid
        .context("Microsoft ID-token header does not contain a key ID")?;
    let signing_key = signing_keys
        .keys
        .iter()
        .find(|key| key.kid == kid)
        .context("Microsoft ID token is not signed with a known signing key")?;
    if signing_key.kty != "RSA" {
        anyhow::bail!("Microsoft ID-token signing key is not an RSA key");
    }

    let decoding_key = DecodingKey::from_rsa_components(&signing_key.n, &signing_key.e)
        .context("failed to construct a decoding key from the Microsoft signing key")?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_required_spec_claims(&["exp", "nbf", "aud", "iss"]);
    validation.validate_nbf = true;
    validation.set_audience(&[expected_audience]);
    validation.set_issuer(&[&signing_keys.issuer]);

    let claims =
        jsonwebtoken::decode::<MicrosoftIdTokenClaims>(id_token, &decoding_key, &validation)
            .context("Microsoft ID token failed signature or claims validation")?
            .claims;

    if claims.tid != expected_tenant_id {
        anyhow::bail!("Microsoft ID-token tenant does not match the configured tenant");
    }
    if claims.sub.trim().is_empty() {
        anyhow::bail!("Microsoft ID token does not contain a subject");
    }
    if claims.oid.trim().is_empty() {
        anyhow::bail!("Microsoft ID token does not contain an object identifier");
    }

    let email = claims
        .email
        .filter(|email| !email.trim().is_empty())
        .or_else(|| {
            claims
                .preferred_username
                .filter(|username| !username.trim().is_empty())
        })
        .context("Microsoft ID token does not contain an email or preferred username")?;

    Ok(MicrosoftUserInfo {
        sub: claims.sub,
        tenant_id: claims.tid,
        object_id: claims.oid,
        email,
    })
}

fn endpoint_url(tenant_id: &str, endpoint: &str) -> anyhow::Result<Url> {
    let mut url =
        Url::parse(MICROSOFT_LOGIN_BASE_URL).context("invalid Microsoft OAuth base URL")?;
    url.path_segments_mut()
        .expect("Microsoft OAuth base URL must support path segments")
        .pop_if_empty()
        .extend([tenant_id, "oauth2", "v2.0", endpoint]);
    Ok(url)
}

fn discovery_url(login_base_url: &str, tenant_id: &str) -> anyhow::Result<Url> {
    let mut url = Url::parse(login_base_url).context("invalid Microsoft OAuth base URL")?;
    url.path_segments_mut()
        .map_err(|()| anyhow::anyhow!("Microsoft OAuth base URL must support path segments"))?
        .pop_if_empty()
        .extend([tenant_id, "v2.0", ".well-known", "openid-configuration"]);
    Ok(url)
}

fn generic_error(message: impl Into<String>) -> FusionAuthClientError {
    FusionAuthClientError::Generic(GenericErrorResponse {
        message: message.into(),
    })
}
