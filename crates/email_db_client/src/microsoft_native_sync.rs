use models_email::email::service::{message::Message, microsoft::MicrosoftFolderSync};
use serde_json::Value;
use sqlx::{PgPool, Row, types::Uuid};

#[cfg(test)]
mod test;

#[derive(Debug, Clone)]
pub struct MicrosoftFolderWalkState {
    pub cursor: Option<Value>,
    pub generation: i64,
}

pub async fn load_folder_walk_state_for_owner(
    pool: &PgPool,
    owner: &str,
    link_id: Uuid,
) -> anyhow::Result<Option<MicrosoftFolderWalkState>> {
    let row = sqlx::query(
        r#"SELECT m.folder_walk_cursor, m.folder_walk_generation, m.folder_walk_seen_ids
           FROM email_microsoft_mailboxes m
           JOIN email_links l ON l.id = m.link_id
           WHERE m.link_id = $1 AND l.fusionauth_user_id = $2
             AND l.provider = 'MICROSOFT'::email_user_provider_enum
             AND l.is_sync_active = true AND m.disconnected_at IS NULL"#,
    )
    .bind(link_id)
    .bind(owner)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| MicrosoftFolderWalkState {
        cursor: row.get("folder_walk_cursor"),
        generation: row.get("folder_walk_generation"),
    }))
}

/// CAS-persist one bounded traversal step. On completion, folders not observed in the complete
/// traversal are tombstoned in the same transaction before the walk state is cleared.
pub async fn commit_folder_walk_step_for_owner(
    pool: &PgPool,
    owner: &str,
    link_id: Uuid,
    expected_generation: i64,
    next_cursor: Option<Value>,
    discovered_ids: &[String],
) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    if !crate::microsoft_mailbox::lock_active_microsoft_mailbox_for_owner(&mut tx, owner, link_id)
        .await?
    {
        tx.rollback().await?;
        return Ok(false);
    }
    let row = sqlx::query(
        r#"UPDATE email_microsoft_mailboxes m
           SET folder_walk_cursor = $4,
               folder_walk_seen_ids = CASE WHEN m.folder_walk_started_at IS NULL
                   THEN $5::text[] ELSE ARRAY(SELECT DISTINCT unnest(m.folder_walk_seen_ids || $5::text[])) END,
               folder_walk_started_at = COALESCE(m.folder_walk_started_at, now()),
               folder_walk_generation = m.folder_walk_generation + 1,
               updated_at = now()
           FROM email_links l
           WHERE m.link_id = $1 AND l.id = m.link_id AND l.fusionauth_user_id = $2
             AND m.folder_walk_generation = $3
             AND l.provider = 'MICROSOFT'::email_user_provider_enum
             AND l.is_sync_active = true AND m.disconnected_at IS NULL
           RETURNING m.folder_walk_seen_ids"#,
    )
    .bind(link_id)
    .bind(owner)
    .bind(expected_generation)
    .bind(next_cursor.clone())
    .bind(discovered_ids)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.rollback().await?;
        return Ok(false);
    };

    if next_cursor.is_none() {
        let seen: Vec<String> = row.get("folder_walk_seen_ids");
        sqlx::query(
            r#"UPDATE email_microsoft_folder_sync f
               SET is_deleted = true, delta_cursor = NULL,
                   cursor_generation = cursor_generation + 1, updated_at = now()
               FROM email_links l
               WHERE f.link_id = $1 AND l.id = f.link_id AND l.fusionauth_user_id = $2
                 AND NOT (f.folder_id = ANY($3::text[])) AND f.is_deleted = false"#,
        )
        .bind(link_id)
        .bind(owner)
        .bind(&seen)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"UPDATE email_microsoft_mailboxes
               SET folder_walk_cursor = NULL, folder_walk_seen_ids = '{}',
                   folder_walk_started_at = NULL, updated_at = now()
               WHERE link_id = $1"#,
        )
        .bind(link_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(true)
}

pub async fn fetch_folder_for_owner(
    pool: &PgPool,
    owner: &str,
    link_id: Uuid,
    folder_id: &str,
) -> anyhow::Result<Option<MicrosoftFolderSync>> {
    let rows = crate::microsoft_folder_sync::list_microsoft_folders_for_owner(pool, owner, link_id)
        .await?;
    Ok(rows
        .into_iter()
        .find(|folder| folder.folder_id == folder_id))
}

/// Projects a provider message using the existing native email thread/message/contact/attachment
/// tables, then records its current Outlook folder. Replays are idempotent by provider IDs.
pub async fn upsert_projected_message_for_owner(
    pool: &PgPool,
    owner: &str,
    folder_id: &str,
    message: &mut Message,
) -> anyhow::Result<bool> {
    let mut guard_tx = pool.begin().await?;
    if !crate::microsoft_mailbox::lock_active_microsoft_mailbox_for_owner(
        &mut guard_tx,
        owner,
        message.link_id,
    )
    .await?
    {
        guard_tx.rollback().await?;
        return Ok(false);
    }
    let provider_thread_id = message
        .provider_thread_id
        .clone()
        .ok_or_else(|| anyhow::anyhow!("Microsoft message has no provider thread identity"))?;
    let provider_message_id = message
        .provider_id
        .clone()
        .ok_or_else(|| anyhow::anyhow!("Microsoft message has no provider message identity"))?;
    // Contacts are shared rows and the existing insertion contract intentionally prepares them
    // outside the message transaction. The mailbox row locks remain held while that happens, so
    // disconnect cannot complete between the capability check and the durable projection.
    let addresses = crate::parse::service_to_db::addresses_from_message(message);
    let recipients = crate::contacts::upsert_message::parse_and_upsert_message_contacts(
        pool,
        message.link_id,
        addresses,
    )
    .await?;
    let thread = models_email::email::service::thread::Thread {
        db_id: macro_uuid::generate_uuid_v7(),
        provider_id: Some(provider_thread_id.clone()),
        link_id: message.link_id,
        inbox_visible: false,
        is_read: false,
        latest_inbound_message_ts: None,
        latest_outbound_message_ts: None,
        latest_non_spam_message_ts: None,
        created_at: Default::default(),
        updated_at: Default::default(),
        messages: vec![],
    };
    let thread_id =
        crate::threads::insert::insert_thread(&mut guard_tx, &thread, message.link_id).await?;
    message.thread_db_id = thread_id;
    crate::messages::insert::insert_message_with_tx(
        &mut guard_tx,
        thread_id,
        message,
        message.link_id,
        recipients,
        true,
    )
    .await?;
    let message_id: Uuid =
        sqlx::query_scalar("SELECT id FROM email_messages WHERE link_id = $1 AND provider_id = $2")
            .bind(message.link_id)
            .bind(&provider_message_id)
            .fetch_one(&mut *guard_tx)
            .await?;
    sqlx::query(
        r#"INSERT INTO email_microsoft_message_state
           (link_id, provider_message_id, message_id, folder_id, change_key, provider_thread_id)
           VALUES ($1, $2, $3, $4, $5, $6)
           ON CONFLICT (link_id, provider_message_id) DO UPDATE SET
             message_id = EXCLUDED.message_id, folder_id = EXCLUDED.folder_id,
             change_key = EXCLUDED.change_key, provider_thread_id = EXCLUDED.provider_thread_id,
             updated_at = now()"#,
    )
    .bind(message.link_id)
    .bind(provider_message_id)
    .bind(message_id)
    .bind(folder_id)
    .bind(message.provider_history_id.clone())
    .bind(provider_thread_id)
    .execute(&mut *guard_tx)
    .await?;
    guard_tx.commit().await?;
    Ok(true)
}

/// Applies a Graph tombstone only when the message is still projected in the source folder.
/// This makes old-folder tombstones harmless when the new-folder upsert won the race first.
pub async fn remove_projected_message_from_folder_for_owner(
    pool: &PgPool,
    owner: &str,
    link_id: Uuid,
    folder_id: &str,
    provider_message_id: &str,
) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    if !crate::microsoft_mailbox::lock_active_microsoft_mailbox_for_owner(&mut tx, owner, link_id)
        .await?
    {
        tx.rollback().await?;
        return Ok(false);
    }
    let row = sqlx::query(
        r#"SELECT s.message_id, m.thread_id
           FROM email_microsoft_message_state s
           JOIN email_messages m ON m.id = s.message_id
           JOIN email_links l ON l.id = s.link_id
           WHERE s.link_id = $1 AND s.provider_message_id = $2 AND s.folder_id = $3
             AND l.fusionauth_user_id = $4 AND l.provider = 'MICROSOFT'::email_user_provider_enum
           FOR UPDATE OF s"#,
    )
    .bind(link_id)
    .bind(provider_message_id)
    .bind(folder_id)
    .bind(owner)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.rollback().await?;
        return Ok(false);
    };
    let message_id: Uuid = row.get("message_id");
    let thread_id: Uuid = row.get("thread_id");
    sqlx::query("DELETE FROM email_messages WHERE id = $1 AND link_id = $2")
        .bind(message_id)
        .bind(link_id)
        .execute(&mut *tx)
        .await?;
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM email_messages WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&mut *tx)
            .await?;
    if remaining == 0 {
        sqlx::query("DELETE FROM email_threads WHERE id = $1 AND link_id = $2")
            .bind(thread_id)
            .bind(link_id)
            .execute(&mut *tx)
            .await?;
    } else {
        crate::threads::update::update_thread_metadata(&mut *tx, thread_id, link_id).await?;
    }
    tx.commit().await?;
    Ok(true)
}

pub async fn clear_projected_folder_for_owner(
    pool: &PgPool,
    owner: &str,
    link_id: Uuid,
    folder_id: &str,
) -> anyhow::Result<(u64, bool)> {
    let mut tx = pool.begin().await?;
    if !crate::microsoft_mailbox::lock_active_microsoft_mailbox_for_owner(&mut tx, owner, link_id)
        .await?
    {
        tx.rollback().await?;
        return Ok((0, false));
    }

    let rows = sqlx::query(
        r#"SELECT s.message_id, m.thread_id
           FROM email_microsoft_message_state s
           JOIN email_messages m ON m.id = s.message_id AND m.link_id = s.link_id
           WHERE s.link_id = $1 AND s.folder_id = $2
           ORDER BY s.updated_at, s.provider_message_id
           LIMIT 500
           FOR UPDATE OF s"#,
    )
    .bind(link_id)
    .bind(folder_id)
    .fetch_all(&mut *tx)
    .await?;

    let mut thread_ids = std::collections::HashSet::new();
    for row in &rows {
        let message_id: Uuid = row.get("message_id");
        let thread_id: Uuid = row.get("thread_id");
        thread_ids.insert(thread_id);
        sqlx::query("DELETE FROM email_messages WHERE id = $1 AND link_id = $2")
            .bind(message_id)
            .bind(link_id)
            .execute(&mut *tx)
            .await?;
    }

    for thread_id in thread_ids {
        let remaining: i64 =
            sqlx::query_scalar("SELECT count(*) FROM email_messages WHERE thread_id = $1")
                .bind(thread_id)
                .fetch_one(&mut *tx)
                .await?;
        if remaining == 0 {
            sqlx::query("DELETE FROM email_threads WHERE id = $1 AND link_id = $2")
                .bind(thread_id)
                .bind(link_id)
                .execute(&mut *tx)
                .await?;
        } else {
            crate::threads::update::update_thread_metadata(&mut *tx, thread_id, link_id).await?;
        }
    }

    let has_more: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM email_microsoft_message_state
           WHERE link_id = $1 AND folder_id = $2)"#,
    )
    .bind(link_id)
    .bind(folder_id)
    .fetch_one(&mut *tx)
    .await?;
    let removed = rows.len() as u64;
    tx.commit().await?;
    Ok((removed, has_more))
}

pub async fn list_deleted_folder_ids_for_owner(
    pool: &PgPool,
    owner: &str,
    link_id: Uuid,
) -> anyhow::Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        r#"SELECT f.folder_id FROM email_microsoft_folder_sync f
           JOIN email_links l ON l.id = f.link_id
           WHERE f.link_id = $1 AND l.fusionauth_user_id = $2
             AND l.provider = 'MICROSOFT'::email_user_provider_enum
             AND f.is_deleted = true"#,
    )
    .bind(link_id)
    .bind(owner)
    .fetch_all(pool)
    .await?)
}
