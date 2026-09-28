use crate::api::context::{ApiContext, AuthorizationService};
use axum::{Json, extract::State, http::StatusCode, response::{IntoResponse, Response}};
use macro_authorization::{MacroAuthorizationExtractor, UserOrInternal};
use macro_user_id::{cowlike::CowLike, email::EmailStr};
use model::response::ErrorResponse;
use models_email::email::service::{
    microsoft::NewMicrosoftMailbox,
    pubsub::{LinkManagerMessage, MicrosoftSyncOperation},
};
use models_email::service::link::{Link, UserProvider};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrosoftMailboxInitResponse {
    pub link_id: Uuid,
    pub email: String,
    pub created: bool,
}

/// Creates or reactivates the caller's Microsoft link from auth-service verified identity.
/// No caller-supplied email, tenant, or object id is accepted as ownership proof.
pub async fn init_handler(
    State(ctx): State<ApiContext>,
    authorization: MacroAuthorizationExtractor<AuthorizationService, UserOrInternal>,
) -> Result<Json<MicrosoftMailboxInitResponse>, Response> {
    if !cfg!(feature = "microsoft_graph_readonly") {
        return Err(error(StatusCode::SERVICE_UNAVAILABLE, "Microsoft mailbox sync is disabled"));
    }
    let user = &authorization.authorization.user;
    let owner = &user.user_context.fusion_user_id;
    let status = ctx.auth_service_client.get_microsoft_mailbox_status(owner).await.map_err(|e| {
        tracing::warn!(error=?e, "failed to load Microsoft mailbox status");
        error(StatusCode::BAD_GATEWAY, "Microsoft mailbox status is unavailable")
    })?;
    let mailbox = status.mailbox.filter(|_| status.connected).ok_or_else(|| {
        error(StatusCode::CONFLICT, "Microsoft mailbox authorization is not connected")
    })?;
    let normalized = mailbox.email.to_ascii_lowercase();
    let email = EmailStr::try_from(normalized.clone()).map_err(|_| {
        error(StatusCode::BAD_GATEWAY, "Microsoft returned an invalid mailbox address")
    })?;
    let requested_id = macro_uuid::generate_uuid_v7();
    let macro_id = user.macro_user_id.clone().into_owned();
    let link = Link {
        id: requested_id,
        is_primary: Link::derive_is_primary(&macro_id, &email),
        macro_id,
        fusionauth_user_id: owner.clone(),
        email_address: email,
        provider: UserProvider::Microsoft,
        is_sync_active: true,
        needs_reauth: false,
        last_sync_error_at: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let (persisted, _) = email_db_client::microsoft_mailbox::provision_microsoft_mailbox(
        &ctx.db,
        link,
        NewMicrosoftMailbox {
            link_id: requested_id,
            tenant_id: mailbox.tenant_id,
            mailbox_id: mailbox.object_id,
            user_principal_name: normalized.clone(),
        },
    )
    .await
    .map_err(|e| {
        tracing::warn!(error=?e, "failed to provision Microsoft mailbox");
        error(StatusCode::INTERNAL_SERVER_ERROR, "Unable to provision Microsoft mailbox")
    })?;
    ctx.sqs_client
        .enqueue_link_manager_notification(LinkManagerMessage::MicrosoftSync {
            link_id: persisted.id,
            sync_operation: MicrosoftSyncOperation::DiscoverFolders,
        })
        .await
        .map_err(|e| {
            tracing::warn!(error=?e, link_id=%persisted.id, "failed to enqueue Microsoft mailbox initialization");
            error(StatusCode::SERVICE_UNAVAILABLE, "Microsoft mailbox sync could not be scheduled")
        })?;
    Ok(Json(MicrosoftMailboxInitResponse {
        link_id: persisted.id,
        email: normalized,
        created: persisted.id == requested_id,
    }))
}

fn error(status: StatusCode, message: &'static str) -> Response {
    (status, Json(ErrorResponse { message: message.into() })).into_response()
}
