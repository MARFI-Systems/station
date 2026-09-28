use super::context::LinkManagerContext;
use crate::outbound::email_api::MicrosoftMailboxTokenSource;
use anyhow::Context;
use email_api_client::domain::models::EmailApiError;
use email_api_client::outbound::microsoft_graph::{
    MicrosoftGraphBodyContentType, MicrosoftGraphDeltaCursor, MicrosoftGraphDeltaCursorKind,
    MicrosoftGraphEmailAddress, MicrosoftGraphImportance,
    MicrosoftGraphMessage, MicrosoftGraphMessageDeltaChange,
};
use models_email::email::service::address::ContactInfo;
use models_email::email::service::attachment::Attachment;
use models_email::email::service::label::{
    Label, LabelListVisibility, LabelType, MessageListVisibility, system_labels,
};
use models_email::email::service::message::Message;
use models_email::email::service::microsoft::DiscoveredMicrosoftFolder;
use models_email::email::service::pubsub::{LinkManagerMessage, MicrosoftSyncOperation};
use models_email::service::link::{Link, UserProvider};
use uuid::Uuid;

pub async fn process(
    ctx: &LinkManagerContext,
    link: &Link,
    sync_operation: MicrosoftSyncOperation,
) -> anyhow::Result<()> {
    if link.provider != UserProvider::Microsoft || !link.is_sync_active {
        return Ok(());
    }
    let token = MicrosoftMailboxTokenSource::new(
        ctx.db.clone(),
        ctx.auth_service_client.clone(),
        ctx.sqs_client.clone(),
    )
    .get_access_token(link.id)
    .await
    .map_err(anyhow::Error::new)?;

    match sync_operation {
        MicrosoftSyncOperation::DiscoverFolders => discover_folders(ctx, link, &token).await,
        MicrosoftSyncOperation::PurgeFolder { folder_id } => purge_folder(ctx, link, folder_id).await,
        MicrosoftSyncOperation::SyncFolder {
            folder_id,
            expected_generation,
            rebuild,
        } => sync_folder(ctx, link, &token, folder_id, expected_generation, rebuild).await,
    }
}

async fn discover_folders(
    ctx: &LinkManagerContext,
    link: &Link,
    token: &email_api_client::domain::models::AccessToken,
) -> anyhow::Result<()> {
    let state = email_db_client::microsoft_native_sync::load_folder_walk_state_for_owner(
        &ctx.db,
        &link.fusionauth_user_id,
        link.id,
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("Microsoft mailbox is not active for owner"))?;
    if state.cursor.is_none() {
        seed_well_known_folders(ctx, link, token).await?;
    }
    let cursor = state
        .cursor
        .map(serde_json::from_value)
        .transpose()
        .context("invalid persisted Microsoft folder walk cursor")?;
    let page = ctx.microsoft_graph.walk_mail_folders(token, cursor.as_ref()).await?;
    let mut discovered = Vec::with_capacity(page.folders.len());
    for entry in page.folders {
        discovered.push(entry.folder.id.clone());
        email_db_client::microsoft_folder_sync::upsert_microsoft_folder_for_owner(
            &ctx.db,
            &link.fusionauth_user_id,
            DiscoveredMicrosoftFolder {
                link_id: link.id,
                folder_id: entry.folder.id,
                parent_folder_id: entry.folder.parent_folder_id,
                well_known_name: None,
                display_name: entry.folder.display_name,
            },
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("Microsoft folder owner boundary rejected discovery"))?;
    }
    let next = page
        .next_cursor
        .map(serde_json::to_value)
        .transpose()
        .context("failed to serialize Microsoft folder walk cursor")?;
    if !email_db_client::microsoft_native_sync::commit_folder_walk_step_for_owner(
        &ctx.db,
        &link.fusionauth_user_id,
        link.id,
        state.generation,
        next.clone(),
        &discovered,
    )
    .await?
    {
        tracing::debug!(link_id=%link.id, "stale Microsoft folder discovery worker lost CAS");
        return Ok(());
    }
    if next.is_some() {
        enqueue(ctx, link.id, MicrosoftSyncOperation::DiscoverFolders).await?;
        return Ok(());
    }
    for folder_id in email_db_client::microsoft_native_sync::list_deleted_folder_ids_for_owner(
        &ctx.db, &link.fusionauth_user_id, link.id,
    ).await? {
        enqueue(ctx, link.id, MicrosoftSyncOperation::PurgeFolder { folder_id }).await?;
    }
    for folder in email_db_client::microsoft_folder_sync::list_microsoft_folders_for_owner(
        &ctx.db,
        &link.fusionauth_user_id,
        link.id,
    )
    .await?
    {
        enqueue(
            ctx,
            link.id,
            MicrosoftSyncOperation::SyncFolder {
                folder_id: folder.folder_id,
                expected_generation: folder.cursor_generation,
                rebuild: false,
            },
        )
        .await?;
    }
    Ok(())
}

async fn purge_folder(
    ctx: &LinkManagerContext,
    link: &Link,
    folder_id: String,
) -> anyhow::Result<()> {
    let (_, has_more) = email_db_client::microsoft_native_sync::clear_projected_folder_for_owner(
        &ctx.db, &link.fusionauth_user_id, link.id, &folder_id,
    ).await?;
    if has_more {
        enqueue(ctx, link.id, MicrosoftSyncOperation::PurgeFolder { folder_id }).await?;
    }
    Ok(())
}

async fn sync_folder(
    ctx: &LinkManagerContext,
    link: &Link,
    token: &email_api_client::domain::models::AccessToken,
    folder_id: String,
    expected_generation: i64,
    rebuild: bool,
) -> anyhow::Result<()> {
    let Some(folder) = email_db_client::microsoft_native_sync::fetch_folder_for_owner(
        &ctx.db,
        &link.fusionauth_user_id,
        link.id,
        &folder_id,
    )
    .await?
    else {
        return Ok(());
    };
    if folder.cursor_generation != expected_generation {
        tracing::debug!(link_id=%link.id, folder_id, "stale Microsoft folder worker lost CAS");
        return Ok(());
    }
    if rebuild {
        let (_, has_more) = email_db_client::microsoft_native_sync::clear_projected_folder_for_owner(
            &ctx.db,
            &link.fusionauth_user_id,
            link.id,
            &folder_id,
        )
        .await?;
        if has_more {
            enqueue(ctx, link.id, MicrosoftSyncOperation::SyncFolder {
                folder_id, expected_generation, rebuild: true,
            }).await?;
            return Ok(());
        }
    }
    let cursor: Option<MicrosoftGraphDeltaCursor> = folder
        .delta_cursor
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .context("invalid persisted Microsoft delta cursor")?;
    let batch = match ctx
        .microsoft_graph
        .list_folder_message_delta(token, &folder_id, cursor.as_ref())
        .await
    {
        Ok(batch) => batch,
        Err(EmailApiError::OutdatedCursor) if cursor.is_some() => {
            let Some(reset) = email_db_client::microsoft_folder_sync::invalidate_microsoft_folder_cursor_for_owner(
                &ctx.db,
                &link.fusionauth_user_id,
                link.id,
                &folder_id,
                expected_generation,
            )
            .await?
            else {
                return Ok(());
            };
            enqueue(
                ctx,
                link.id,
                MicrosoftSyncOperation::SyncFolder {
                    folder_id,
                    expected_generation: reset.cursor_generation,
                    rebuild: true,
                },
            )
            .await?;
            return Ok(());
        }
        Err(error) => return Err(anyhow::Error::new(error)),
    };

    for change in batch.changes {
        match change {
            MicrosoftGraphMessageDeltaChange::Upsert(delta_message) => {
                let Some(full) = ctx.microsoft_graph.get_message(token, &delta_message.id).await? else {
                    email_db_client::microsoft_native_sync::remove_projected_message_from_folder_for_owner(
                        &ctx.db, &link.fusionauth_user_id, link.id, &folder_id, &delta_message.id,
                    ).await?;
                    continue;
                };
                let actual_folder_id = full.parent_folder_id.as_deref().unwrap_or(&folder_id).to_owned();
                let actual_folder = email_db_client::microsoft_native_sync::fetch_folder_for_owner(
                    &ctx.db, &link.fusionauth_user_id, link.id, &actual_folder_id,
                ).await?.unwrap_or_else(|| folder.clone());
                ensure_labels(&ctx.db, link.id, &actual_folder).await?;
                let mut projected = map_message(link, &actual_folder, full);
                email_db_client::microsoft_native_sync::upsert_projected_message_for_owner(
                    &ctx.db, &link.fusionauth_user_id, &actual_folder_id, &mut projected,
                ).await?;
            }
            MicrosoftGraphMessageDeltaChange::Removed(removed) => {
                email_db_client::microsoft_native_sync::remove_projected_message_from_folder_for_owner(
                    &ctx.db, &link.fusionauth_user_id, link.id, &folder_id, &removed.id,
                ).await?;
            }
        }
    }

    let serialized_cursor = serde_json::to_string(&batch.cursor)?;
    let round_complete = batch.cursor.kind() == MicrosoftGraphDeltaCursorKind::Delta;
    let Some(committed) = email_db_client::microsoft_folder_sync::commit_microsoft_folder_cursor_for_owner(
        &ctx.db,
        &link.fusionauth_user_id,
        link.id,
        &folder_id,
        expected_generation,
        &serialized_cursor,
        round_complete,
    )
    .await?
    else {
        return Ok(());
    };
    if !round_complete {
        enqueue(
            ctx,
            link.id,
            MicrosoftSyncOperation::SyncFolder {
                folder_id,
                expected_generation: committed.cursor_generation,
                rebuild: false,
            },
        )
        .await?;
    }
    Ok(())
}

async fn enqueue(
    ctx: &LinkManagerContext,
    link_id: Uuid,
    sync_operation: MicrosoftSyncOperation,
) -> anyhow::Result<()> {
    ctx.sqs_client
        .enqueue_link_manager_notification(LinkManagerMessage::MicrosoftSync { link_id, sync_operation })
        .await
}

async fn seed_well_known_folders(
    ctx: &LinkManagerContext,
    link: &Link,
    token: &email_api_client::domain::models::AccessToken,
) -> anyhow::Result<()> {
    for well_known_name in ["inbox", "sentitems", "drafts", "deleteditems", "junkemail"] {
        let Some(folder) = ctx.microsoft_graph.get_mail_folder(token, well_known_name).await? else {
            continue;
        };
        email_db_client::microsoft_folder_sync::upsert_microsoft_folder_for_owner(
            &ctx.db,
            &link.fusionauth_user_id,
            DiscoveredMicrosoftFolder {
                link_id: link.id,
                folder_id: folder.id,
                parent_folder_id: folder.parent_folder_id,
                display_name: folder.display_name,
                well_known_name: Some(well_known_name.to_owned()),
            },
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("Microsoft folder owner boundary rejected canonical folder"))?;
    }
    Ok(())
}

async fn ensure_labels(
    pool: &sqlx::PgPool,
    link_id: Uuid,
    folder: &models_email::email::service::microsoft::MicrosoftFolderSync,
) -> anyhow::Result<()> {
    let mut labels = vec![
        folder_label(link_id, folder),
        label(link_id, system_labels::UNREAD, system_labels::UNREAD, LabelType::System),
        label(link_id, system_labels::IMPORTANT, system_labels::IMPORTANT, LabelType::System),
    ];
    if let Some(system) = system_label_id(folder.well_known_name.as_deref()) {
        labels.push(label(link_id, system, system, LabelType::System));
    }
    email_db_client::labels::insert::insert_or_update_labels(pool, labels).await
}

fn folder_label(
    link_id: Uuid,
    folder: &models_email::email::service::microsoft::MicrosoftFolderSync,
) -> Label {
    label(
        link_id,
        &format!("microsoft-folder:{}", folder.folder_id),
        &folder.display_name,
        LabelType::User,
    )
}

fn label(link_id: Uuid, provider_id: &str, name: &str, type_: LabelType) -> Label {
    Label {
        id: None,
        link_id,
        provider_label_id: provider_id.to_owned(),
        name: Some(name.to_owned()),
        created_at: chrono::Utc::now(),
        message_list_visibility: Some(MessageListVisibility::Show),
        label_list_visibility: Some(LabelListVisibility::LabelShow),
        type_: Some(type_),
    }
}

fn system_label_id(well_known: Option<&str>) -> Option<&'static str> {
    match well_known {
        Some("inbox") => Some(system_labels::INBOX),
        Some("sentitems") => Some(system_labels::SENT),
        Some("drafts") => Some(system_labels::DRAFT),
        Some("deleteditems") => Some(system_labels::TRASH),
        Some("junkemail") => Some(system_labels::SPAM),
        _ => None,
    }
}

fn map_message(
    link: &Link,
    folder: &models_email::email::service::microsoft::MicrosoftFolderSync,
    message: MicrosoftGraphMessage,
) -> Message {
    let provider_thread_id = message
        .conversation_id
        .clone()
        .unwrap_or_else(|| format!("message:{}", message.id));
    let mut labels = vec![folder_label(link.id, folder)];
    if let Some(system) = system_label_id(folder.well_known_name.as_deref()) {
        labels.push(label(link.id, system, system, LabelType::System));
    }
    if !message.is_read {
        labels.push(label(link.id, system_labels::UNREAD, system_labels::UNREAD, LabelType::System));
    }
    if message.importance == MicrosoftGraphImportance::High {
        labels.push(label(link.id, system_labels::IMPORTANT, system_labels::IMPORTANT, LabelType::System));
    }
    let (body_text, body_html_sanitized) = match message.body {
        Some(body) => match body.content_type {
            MicrosoftGraphBodyContentType::Html => (None, Some(email_utils::sanitize_email_html(&body.content))),
            MicrosoftGraphBodyContentType::Text | MicrosoftGraphBodyContentType::Other(_) => (Some(body.content), None),
        },
        None => (None, None),
    };
    Message {
        db_id: macro_uuid::generate_uuid_v7(),
        provider_id: Some(message.id),
        thread_db_id: Uuid::nil(),
        provider_thread_id: Some(provider_thread_id),
        replying_to_id: None,
        global_id: message.internet_message_id,
        link_id: link.id,
        subject: message.subject,
        snippet: message.body_preview,
        provider_history_id: message.change_key,
        internal_date_ts: message.received_at.or(message.sent_at),
        sent_at: message.sent_at,
        size_estimate: None,
        is_read: message.is_read,
        is_starred: false,
        is_sent: folder.well_known_name.as_deref() == Some("sentitems"),
        is_draft: message.is_draft,
        scheduled_send_time: None,
        has_attachments: message.has_attachments,
        from: message.from.map(map_address),
        to: message.to_recipients.into_iter().map(map_address).collect(),
        cc: message.cc_recipients.into_iter().map(map_address).collect(),
        bcc: message.bcc_recipients.into_iter().map(map_address).collect(),
        labels,
        body_text,
        body_html_sanitized,
        body_macro: None,
        attachments: message.attachments.into_iter().map(|attachment| Attachment {
            db_id: macro_uuid::generate_uuid_v7(),
            provider_id: Some(attachment.id),
            data_url: None,
            filename: attachment.name,
            mime_type: attachment.content_type,
            size_bytes: attachment.size_bytes.and_then(|size| i64::try_from(size).ok()),
            sfs_id: None,
            content_id: attachment.content_id,
        }).collect(),
        attachments_draft: vec![],
        attachments_forwarded: vec![],
        headers_json: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

fn map_address(address: MicrosoftGraphEmailAddress) -> ContactInfo {
    ContactInfo { email: address.address.to_ascii_lowercase(), name: address.name, photo_url: None }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn only_canonical_well_known_names_define_system_semantics() {
        assert_eq!(system_label_id(Some("inbox")), Some(system_labels::INBOX));
        assert_eq!(system_label_id(Some("sentitems")), Some(system_labels::SENT));
        assert_eq!(system_label_id(Some("Boîte de réception")), None);
        assert_eq!(system_label_id(Some("custom inbox")), None);
    }

    #[test]
    fn normalizes_graph_contacts() {
        let contact = map_address(MicrosoftGraphEmailAddress { name: Some("A".into()), address: "A@EXAMPLE.COM".into() });
        assert_eq!(contact.email, "a@example.com");
    }
}
