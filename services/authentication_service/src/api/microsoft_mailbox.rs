//! Delegated, read-only Microsoft mailbox consent, separate from login identity linking.

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use email_validator::normalize_email;
use macro_authorization::{MacroAuthorizationExtractor, UserOnly};
use model::response::ErrorResponse;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    api::context::{ApiContext, AuthorizationService},
    config::BASE_URL,
    microsoft_token_cipher::{EncryptedMicrosoftToken, MicrosoftRefreshToken},
};

type User = MacroAuthorizationExtractor<AuthorizationService, UserOnly>;
const STATE_TTL_SECONDS: i64 = 600;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrosoftMailboxConnectResponse {
    pub authorization_url: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrosoftMailboxStatus {
    pub connected: bool,
    pub email: Option<String>,
    pub tenant_id: Option<String>,
    pub object_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct MicrosoftMailboxState {
    flow_id: Uuid,
    state_secret: String,
}

#[derive(Deserialize)]
struct CallbackParams {
    code: Option<String>,
    state: String,
    error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
enum MailboxError {
    #[error("Microsoft mailbox connection is unavailable in this deployment")]
    Unavailable,
    #[error("invalid, expired, or already consumed Microsoft mailbox authorization state")]
    InvalidState,
    #[error("Microsoft mailbox authorization was not completed")]
    AuthorizationDenied,
    #[error("Microsoft mailbox is not connected")]
    NotConnected,
    #[error("unable to connect Microsoft mailbox")]
    Internal,
}
impl IntoResponse for MailboxError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::InvalidState | Self::AuthorizationDenied => StatusCode::BAD_REQUEST,
            Self::NotConnected => StatusCode::NOT_FOUND,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status,
            Json(ErrorResponse {
                message: self.to_string().into(),
            }),
        )
            .into_response()
    }
}

pub fn router() -> Router<ApiContext> {
    Router::new()
        .route("/", get(status).delete(disconnect))
        .route("/connect", post(start))
        .route("/callback", get(callback))
}

fn enabled(
    ctx: &ApiContext,
) -> Result<
    (
        &dyn crate::microsoft_token_cipher::MicrosoftTokenCipher,
        &str,
    ),
    MailboxError,
> {
    if !ctx.microsoft_mailbox_oauth_enabled {
        return Err(MailboxError::Unavailable);
    }
    Ok((
        ctx.microsoft_token_cipher
            .as_deref()
            .ok_or(MailboxError::Unavailable)?,
        ctx.microsoft_mailbox_completion_url
            .as_deref()
            .ok_or(MailboxError::Unavailable)?,
    ))
}

async fn start(
    State(ctx): State<ApiContext>,
    user: User,
) -> Result<Json<MicrosoftMailboxConnectResponse>, MailboxError> {
    let (cipher, _) = enabled(&ctx)?;
    let owner = &user.authorization.user_context.fusion_user_id;
    let flow_id = Uuid::now_v7();
    let state_secret = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
    let state_hash = Sha256::digest(state_secret.as_bytes());
    let verifier = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let encrypted = cipher
        .encrypt_oauth_state(owner, &flow_id, MicrosoftRefreshToken::new(verifier))
        .await
        .map_err(|error| {
            tracing::error!(error=?error, "failed to encrypt Microsoft mailbox PKCE verifier");
            MailboxError::Internal
        })?;
    let encrypted = microsoft_oauth_grant_db_utils::EncryptedMicrosoftOAuthGrant::new(
        encrypted.refresh_token_ciphertext,
        encrypted.encrypted_data_key,
        encrypted.nonce,
        i32::from(encrypted.encryption_version),
        encrypted.kms_key_id,
    );
    microsoft_oauth_grant_db_utils::create_microsoft_mailbox_oauth_flow(
        &ctx.db,
        flow_id,
        owner,
        &state_hash,
        &encrypted,
        chrono::Utc::now() + chrono::Duration::seconds(STATE_TTL_SECONDS),
    )
    .await
    .map_err(|error| {
        tracing::error!(error=?error, "failed to store Microsoft mailbox OAuth flow");
        MailboxError::Internal
    })?;
    let state = MicrosoftMailboxState {
        flow_id,
        state_secret,
    };
    let redirect_uri = format!("{}/microsoft-mailbox/callback", *BASE_URL);
    let authorization_url = ctx
        .auth_client
        .construct_microsoft_mailbox_authorize_url(&redirect_uri, &state, &challenge)
        .map_err(|error| {
            tracing::error!(error=?error, "failed to construct Microsoft mailbox authorize URL");
            MailboxError::Unavailable
        })?;
    Ok(Json(MicrosoftMailboxConnectResponse { authorization_url }))
}

async fn callback(
    State(ctx): State<ApiContext>,
    user: User,
    Query(params): Query<CallbackParams>,
) -> Result<Response, MailboxError> {
    let (cipher, completion_url) = enabled(&ctx)?;
    let state: MicrosoftMailboxState =
        serde_json::from_str(&params.state).map_err(|_| MailboxError::InvalidState)?;
    let owner = &user.authorization.user_context.fusion_user_id;
    let state_hash = Sha256::digest(state.state_secret.as_bytes());
    let flow = microsoft_oauth_grant_db_utils::consume_microsoft_mailbox_oauth_flow(
        &ctx.db,
        state.flow_id,
        owner,
        &state_hash,
    )
    .await
    .map_err(|error| {
        tracing::error!(error=?error, "failed to consume Microsoft mailbox OAuth flow");
        MailboxError::Internal
    })?
    .ok_or(MailboxError::InvalidState)?;
    if params.error.is_some() {
        return Ok(completion_redirect(completion_url, "denied"));
    }
    let encrypted = flow.encrypted_code_verifier();
    let envelope = EncryptedMicrosoftToken {
        refresh_token_ciphertext: encrypted.refresh_token_ciphertext().to_vec(),
        encrypted_data_key: encrypted.encrypted_data_key().to_vec(),
        nonce: encrypted.nonce().to_vec(),
        encryption_version: encrypted
            .encryption_version()
            .try_into()
            .map_err(|_| MailboxError::InvalidState)?,
        kms_key_id: encrypted.kms_key_id().to_owned(),
    };
    let verifier = cipher
        .decrypt_oauth_state(owner, &state.flow_id, &envelope)
        .await
        .map_err(|_| MailboxError::InvalidState)?;
    let code = params
        .code
        .as_deref()
        .ok_or(MailboxError::AuthorizationDenied)?;
    let redirect_uri = format!("{}/microsoft-mailbox/callback", *BASE_URL);
    let (exchange, _access) = ctx
        .auth_client
        .exchange_microsoft_mailbox_code_for_tokens(&redirect_uri, code, verifier.as_str())
        .await
        .map_err(|error| {
            tracing::error!(error=?error, "Microsoft mailbox code exchange failed");
            MailboxError::Internal
        })?;
    let identity = ctx
        .auth_client
        .parse_microsoft_id_token(&exchange.id_token)
        .await
        .map_err(|error| {
            tracing::error!(error=?error, "Microsoft mailbox identity validation failed");
            MailboxError::Internal
        })?;
    let email = normalize_email(&identity.email)
        .map(std::borrow::Cow::into_owned)
        .ok_or(MailboxError::Internal)?;
    let envelope = cipher
        .encrypt(
            owner,
            &email,
            MicrosoftRefreshToken::new(exchange.refresh_token),
        )
        .await
        .map_err(|error| {
            tracing::error!(error=?error, "failed to encrypt Microsoft mailbox grant");
            MailboxError::Internal
        })?;
    let grant = microsoft_oauth_grant_db_utils::EncryptedMicrosoftOAuthGrant::new(
        envelope.refresh_token_ciphertext,
        envelope.encrypted_data_key,
        envelope.nonce,
        i32::from(envelope.encryption_version),
        envelope.kms_key_id,
    );
    microsoft_oauth_grant_db_utils::finalize_microsoft_mailbox_oauth_flow(
        &ctx.db,
        state.flow_id,
        owner,
        &email,
        &identity.tenant_id,
        &identity.object_id,
        &grant,
    )
    .await
    .map_err(|error| {
        tracing::error!(error=?error, "failed to finalize Microsoft mailbox grant");
        MailboxError::Internal
    })?
    .ok_or(MailboxError::InvalidState)?;
    Ok(completion_redirect(completion_url, "connected"))
}

fn completion_redirect(base: &str, result: &str) -> Response {
    let separator = if base.contains('?') { '&' } else { '?' };
    Redirect::to(&format!("{base}{separator}microsoftMailbox={result}")).into_response()
}

async fn status(
    State(ctx): State<ApiContext>,
    user: User,
) -> Result<Json<MicrosoftMailboxStatus>, MailboxError> {
    enabled(&ctx)?;
    let owner = &user.authorization.user_context.fusion_user_id;
    Ok(Json(status_for_owner(&ctx, owner).await?))
}

async fn status_for_owner(
    ctx: &ApiContext,
    owner: &str,
) -> Result<MicrosoftMailboxStatus, MailboxError> {
    let grant = microsoft_oauth_grant_db_utils::get_active_microsoft_mailbox_grant(&ctx.db, owner)
        .await
        .map_err(|error| {
            tracing::error!(error=?error, "failed to load Microsoft mailbox status");
            MailboxError::Internal
        })?;
    Ok(match grant {
        Some(grant) => MicrosoftMailboxStatus {
            connected: true,
            email: Some(grant.email_address().to_owned()),
            tenant_id: grant.microsoft_tenant_id().map(str::to_owned),
            object_id: grant.microsoft_object_id().map(str::to_owned),
        },
        None => MicrosoftMailboxStatus {
            connected: false,
            email: None,
            tenant_id: None,
            object_id: None,
        },
    })
}

async fn disconnect(State(ctx): State<ApiContext>, user: User) -> Result<StatusCode, MailboxError> {
    enabled(&ctx)?;
    let owner = &user.authorization.user_context.fusion_user_id;
    if microsoft_oauth_grant_db_utils::disconnect_microsoft_mailbox_grant(&ctx.db, owner)
        .await
        .map_err(|error| {
            tracing::error!(error=?error, "failed to disconnect Microsoft mailbox grant");
            MailboxError::Internal
        })?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(MailboxError::NotConnected)
    }
}

#[cfg(test)]
mod test;
