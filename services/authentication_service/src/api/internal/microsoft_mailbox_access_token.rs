use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use fusionauth::error::FusionAuthClientError;
use macro_authorization::{InternalOnly, MacroAuthorizationExtractor};
use model::response::ErrorResponse;
use serde::{Deserialize, Serialize};

use crate::{
    api::context::{ApiContext, AuthorizationService},
    microsoft_token_cipher::{EncryptedMicrosoftToken, MicrosoftRefreshToken},
};

#[derive(Deserialize)]
pub struct OwnerParams {
    fusionauth_user_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrosoftMailboxIdentity {
    pub email: String,
    pub tenant_id: String,
    pub object_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrosoftMailboxAccessToken {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
    pub mailbox: MicrosoftMailboxIdentity,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrosoftMailboxStatus {
    pub connected: bool,
    pub mailbox: Option<MicrosoftMailboxIdentity>,
}

type Internal = MacroAuthorizationExtractor<AuthorizationService, InternalOnly>;

pub async fn handler(
    State(ctx): State<ApiContext>,
    _authorization: Internal,
    Query(params): Query<OwnerParams>,
) -> Result<Json<MicrosoftMailboxAccessToken>, Response> {
    available(&ctx)?;
    let stored = active_grant(&ctx, &params.fusionauth_user_id).await?;
    let current = stored.encrypted_grant();
    let envelope = to_envelope(current)?;
    let refresh = ctx
        .microsoft_token_cipher
        .as_ref()
        .expect("checked by available")
        .decrypt(
            &params.fusionauth_user_id,
            stored.email_address(),
            &envelope,
        )
        .await
        .map_err(|e| {
            tracing::error!(error=?e, "failed to decrypt Microsoft mailbox grant");
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "unable to decrypt Microsoft mailbox grant",
            )
        })?;
    let token = match ctx
        .auth_client
        .refresh_microsoft_mailbox_access_token(refresh.as_str())
        .await
    {
        Ok(token) => token,
        Err(FusionAuthClientError::InvalidGrant) => {
            let _ = microsoft_oauth_grant_db_utils::delete_microsoft_oauth_grant_if_current(
                &ctx.db,
                &params.fusionauth_user_id,
                stored.email_address(),
                current,
            )
            .await;
            return Err(error(
                StatusCode::FORBIDDEN,
                "Microsoft mailbox authorization has expired or been revoked",
            ));
        }
        Err(e) => {
            tracing::error!(error=?e, "failed to refresh Microsoft mailbox access token");
            return Err(error(
                StatusCode::BAD_GATEWAY,
                "unable to refresh Microsoft mailbox access token",
            ));
        }
    };
    let still_owned = if let Some(rotated) = token.refresh_token.as_ref() {
        let encrypted = match ctx
            .microsoft_token_cipher
            .as_ref()
            .expect("checked by available")
            .encrypt(
                &params.fusionauth_user_id,
                stored.email_address(),
                MicrosoftRefreshToken::new(rotated.clone()),
            )
            .await
        {
            Ok(encrypted) => encrypted,
            Err(e) => {
                tracing::error!(error=?e, "failed to encrypt rotated Microsoft mailbox grant");
                let _ = microsoft_oauth_grant_db_utils::delete_microsoft_oauth_grant_if_current(
                    &ctx.db,
                    &params.fusionauth_user_id,
                    stored.email_address(),
                    current,
                )
                .await;
                return Err(error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "unable to rotate Microsoft mailbox grant",
                ));
            }
        };
        let replacement = microsoft_oauth_grant_db_utils::EncryptedMicrosoftOAuthGrant::new(
            encrypted.refresh_token_ciphertext,
            encrypted.encrypted_data_key,
            encrypted.nonce,
            i32::from(encrypted.encryption_version),
            encrypted.kms_key_id,
        );
        match microsoft_oauth_grant_db_utils::replace_microsoft_oauth_grant_if_current(
            &ctx.db,
            &params.fusionauth_user_id,
            stored.email_address(),
            current,
            &replacement,
        )
        .await
        {
            Ok(replaced) => replaced,
            Err(e) => {
                tracing::error!(error=?e, "failed to persist rotated Microsoft mailbox grant");
                let _ = microsoft_oauth_grant_db_utils::delete_microsoft_oauth_grant_if_current(
                    &ctx.db,
                    &params.fusionauth_user_id,
                    stored.email_address(),
                    current,
                )
                .await;
                return Err(error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "unable to rotate Microsoft mailbox grant",
                ));
            }
        }
    } else {
        microsoft_oauth_grant_db_utils::microsoft_mailbox_grant_is_current(
            &ctx.db,
            &params.fusionauth_user_id,
            current,
        )
        .await
        .map_err(|e| {
            tracing::error!(error=?e, "failed to confirm Microsoft mailbox ownership");
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "unable to confirm Microsoft mailbox grant",
            )
        })?
    };
    if !still_owned {
        return Err(error(
            StatusCode::NOT_FOUND,
            "Microsoft mailbox grant is disconnected",
        ));
    }
    Ok(Json(MicrosoftMailboxAccessToken {
        access_token: token.access_token,
        token_type: token.token_type,
        expires_in: token.expires_in,
        mailbox: identity(&stored)?,
    }))
}

pub async fn status_handler(
    State(ctx): State<ApiContext>,
    _authorization: Internal,
    Query(params): Query<OwnerParams>,
) -> Result<Json<MicrosoftMailboxStatus>, Response> {
    available(&ctx)?;
    let grant = microsoft_oauth_grant_db_utils::get_active_microsoft_mailbox_grant(
        &ctx.db,
        &params.fusionauth_user_id,
    )
    .await
    .map_err(|e| {
        tracing::error!(error=?e, "failed to load Microsoft mailbox status");
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "unable to load Microsoft mailbox status",
        )
    })?;
    Ok(Json(MicrosoftMailboxStatus {
        connected: grant.is_some(),
        mailbox: grant.as_ref().map(identity).transpose()?,
    }))
}

pub async fn disconnect_handler(
    State(ctx): State<ApiContext>,
    _authorization: Internal,
    Query(params): Query<OwnerParams>,
) -> Result<StatusCode, Response> {
    available(&ctx)?;
    if microsoft_oauth_grant_db_utils::disconnect_microsoft_mailbox_grant(
        &ctx.db,
        &params.fusionauth_user_id,
    )
    .await
    .map_err(|e| {
        tracing::error!(error=?e, "failed to delete local Microsoft mailbox grant");
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "unable to disconnect Microsoft mailbox grant",
        )
    })? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(error(
            StatusCode::NOT_FOUND,
            "Microsoft mailbox grant not found",
        ))
    }
}

fn available(ctx: &ApiContext) -> Result<(), Response> {
    if !ctx.microsoft_mailbox_oauth_enabled || ctx.microsoft_token_cipher.is_none() {
        return Err(error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Microsoft mailbox connection is unavailable",
        ));
    }
    Ok(())
}

async fn active_grant(
    ctx: &ApiContext,
    owner: &str,
) -> Result<microsoft_oauth_grant_db_utils::StoredMicrosoftOAuthGrant, Response> {
    microsoft_oauth_grant_db_utils::get_active_microsoft_mailbox_grant(&ctx.db, owner)
        .await
        .map_err(|e| {
            tracing::error!(error=?e, "failed to load Microsoft mailbox grant");
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "unable to load Microsoft mailbox grant",
            )
        })?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "Microsoft mailbox grant not found"))
}

fn identity(
    stored: &microsoft_oauth_grant_db_utils::StoredMicrosoftOAuthGrant,
) -> Result<MicrosoftMailboxIdentity, Response> {
    Ok(MicrosoftMailboxIdentity {
        email: stored.email_address().to_owned(),
        tenant_id: stored
            .microsoft_tenant_id()
            .ok_or_else(|| {
                error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Microsoft mailbox grant has no tenant identity",
                )
            })?
            .to_owned(),
        object_id: stored
            .microsoft_object_id()
            .ok_or_else(|| {
                error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Microsoft mailbox grant has no object identity",
                )
            })?
            .to_owned(),
    })
}

fn to_envelope(
    current: &microsoft_oauth_grant_db_utils::EncryptedMicrosoftOAuthGrant,
) -> Result<EncryptedMicrosoftToken, Response> {
    Ok(EncryptedMicrosoftToken {
        refresh_token_ciphertext: current.refresh_token_ciphertext().to_vec(),
        encrypted_data_key: current.encrypted_data_key().to_vec(),
        nonce: current.nonce().to_vec(),
        encryption_version: current.encryption_version().try_into().map_err(|_| {
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "invalid Microsoft mailbox grant",
            )
        })?,
        kms_key_id: current.kms_key_id().to_owned(),
    })
}

fn error(status: StatusCode, message: &'static str) -> Response {
    (
        status,
        Json(ErrorResponse {
            message: message.into(),
        }),
    )
        .into_response()
}
