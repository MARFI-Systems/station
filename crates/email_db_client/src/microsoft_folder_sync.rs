use models_email::email::service::microsoft::{DiscoveredMicrosoftFolder, MicrosoftFolderSync};
use sqlx::PgPool;
use sqlx::types::Uuid;

#[cfg(test)]
mod test;

/// Idempotently persist folder metadata inside the mailbox owner boundary.
#[tracing::instrument(err, skip(pool, folder), fields(link_id = %folder.link_id, folder_id = %folder.folder_id))]
pub async fn upsert_microsoft_folder_for_owner(
    pool: &PgPool,
    owner_fusionauth_user_id: &str,
    folder: DiscoveredMicrosoftFolder,
) -> anyhow::Result<Option<MicrosoftFolderSync>> {
    if folder.folder_id.is_empty() || folder.display_name.is_empty() {
        anyhow::bail!("Microsoft folder ID and display name cannot be empty");
    }

    let mut tx = pool.begin().await?;
    if !crate::microsoft_mailbox::lock_active_microsoft_mailbox_for_owner(
        &mut tx,
        owner_fusionauth_user_id,
        folder.link_id,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(None);
    }

    let row = sqlx::query_as!(
        MicrosoftFolderSync,
        r#"
        INSERT INTO email_microsoft_folder_sync (
            link_id, folder_id, parent_folder_id, display_name, well_known_name
        )
        SELECT m.link_id, $3, $4, $5, $6
        FROM email_microsoft_mailboxes m
        JOIN email_links l ON l.id = m.link_id
        WHERE m.link_id = $1
          AND l.fusionauth_user_id = $2
          AND l.provider = 'MICROSOFT'::email_user_provider_enum
          AND l.is_sync_active = true
          AND m.disconnected_at IS NULL
        ON CONFLICT (link_id, folder_id) DO UPDATE SET
            parent_folder_id = EXCLUDED.parent_folder_id,
            display_name = EXCLUDED.display_name,
            well_known_name = COALESCE(EXCLUDED.well_known_name, email_microsoft_folder_sync.well_known_name),
            is_deleted = false,
            updated_at = now()
        RETURNING link_id, folder_id, parent_folder_id, display_name,
                  well_known_name, delta_cursor, cursor_generation,
                  initial_sync_complete, cursor_invalidated_at,
                  last_successful_sync_at, is_deleted, created_at, updated_at
        "#,
        folder.link_id,
        owner_fusionauth_user_id,
        folder.folder_id,
        folder.parent_folder_id,
        folder.display_name,
        folder.well_known_name,
    )
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(row)
}

/// List live folders for one active owner-scoped mailbox.
#[tracing::instrument(err, skip(pool))]
pub async fn list_microsoft_folders_for_owner(
    pool: &PgPool,
    owner_fusionauth_user_id: &str,
    link_id: Uuid,
) -> anyhow::Result<Vec<MicrosoftFolderSync>> {
    let rows = sqlx::query_as!(
        MicrosoftFolderSync,
        r#"
        SELECT f.link_id, f.folder_id, f.parent_folder_id, f.display_name,
               f.well_known_name, f.delta_cursor, f.cursor_generation,
               f.initial_sync_complete, f.cursor_invalidated_at,
               f.last_successful_sync_at, f.is_deleted, f.created_at, f.updated_at
        FROM email_microsoft_folder_sync f
        JOIN email_microsoft_mailboxes m ON m.link_id = f.link_id
        JOIN email_links l ON l.id = m.link_id
        WHERE f.link_id = $1
          AND l.fusionauth_user_id = $2
          AND l.provider = 'MICROSOFT'::email_user_provider_enum
          AND l.is_sync_active = true
          AND m.disconnected_at IS NULL
          AND f.is_deleted = false
        ORDER BY f.created_at, f.folder_id
        "#,
        link_id,
        owner_fusionauth_user_id,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// Compare-and-swap a folder delta cursor after a complete, durable page batch.
///
/// A stale worker receives `None`, so restarts and overlapping jobs cannot move
/// the cursor past data they did not persist.
#[tracing::instrument(err, skip(pool, delta_cursor))]
pub async fn commit_microsoft_folder_cursor_for_owner(
    pool: &PgPool,
    owner_fusionauth_user_id: &str,
    link_id: Uuid,
    folder_id: &str,
    expected_generation: i64,
    delta_cursor: &str,
    initial_sync_complete: bool,
) -> anyhow::Result<Option<MicrosoftFolderSync>> {
    if delta_cursor.is_empty() {
        anyhow::bail!("Microsoft delta cursor cannot be empty");
    }

    let mut tx = pool.begin().await?;
    if !crate::microsoft_mailbox::lock_active_microsoft_mailbox_for_owner(
        &mut tx,
        owner_fusionauth_user_id,
        link_id,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(None);
    }

    let row = sqlx::query_as!(
        MicrosoftFolderSync,
        r#"
        UPDATE email_microsoft_folder_sync f
        SET delta_cursor = $5,
            cursor_generation = f.cursor_generation + 1,
            initial_sync_complete = $6,
            cursor_invalidated_at = NULL,
            last_successful_sync_at = now(),
            updated_at = now()
        FROM email_microsoft_mailboxes m, email_links l
        WHERE f.link_id = $1
          AND f.folder_id = $2
          AND f.cursor_generation = $3
          AND l.fusionauth_user_id = $4
          AND m.link_id = f.link_id
          AND l.id = f.link_id
          AND l.provider = 'MICROSOFT'::email_user_provider_enum
          AND l.is_sync_active = true
          AND m.disconnected_at IS NULL
          AND f.is_deleted = false
        RETURNING f.link_id, f.folder_id, f.parent_folder_id, f.display_name,
                  f.well_known_name, f.delta_cursor, f.cursor_generation,
                  f.initial_sync_complete, f.cursor_invalidated_at,
                  f.last_successful_sync_at, f.is_deleted, f.created_at, f.updated_at
        "#,
        link_id,
        folder_id,
        expected_generation,
        owner_fusionauth_user_id,
        delta_cursor,
        initial_sync_complete,
    )
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(row)
}

/// Clear an expired delta cursor with the same generation guard used by commits.
#[tracing::instrument(err, skip(pool))]
pub async fn invalidate_microsoft_folder_cursor_for_owner(
    pool: &PgPool,
    owner_fusionauth_user_id: &str,
    link_id: Uuid,
    folder_id: &str,
    expected_generation: i64,
) -> anyhow::Result<Option<MicrosoftFolderSync>> {
    let mut tx = pool.begin().await?;
    if !crate::microsoft_mailbox::lock_active_microsoft_mailbox_for_owner(
        &mut tx,
        owner_fusionauth_user_id,
        link_id,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(None);
    }

    let row = sqlx::query_as!(
        MicrosoftFolderSync,
        r#"
        UPDATE email_microsoft_folder_sync f
        SET delta_cursor = NULL,
            cursor_generation = f.cursor_generation + 1,
            initial_sync_complete = false,
            cursor_invalidated_at = now(),
            updated_at = now()
        FROM email_microsoft_mailboxes m, email_links l
        WHERE f.link_id = $1
          AND f.folder_id = $2
          AND f.cursor_generation = $3
          AND l.fusionauth_user_id = $4
          AND m.link_id = f.link_id
          AND l.id = f.link_id
          AND l.provider = 'MICROSOFT'::email_user_provider_enum
          AND l.is_sync_active = true
          AND m.disconnected_at IS NULL
          AND f.is_deleted = false
        RETURNING f.link_id, f.folder_id, f.parent_folder_id, f.display_name,
                  f.well_known_name, f.delta_cursor, f.cursor_generation,
                  f.initial_sync_complete, f.cursor_invalidated_at,
                  f.last_successful_sync_at, f.is_deleted, f.created_at, f.updated_at
        "#,
        link_id,
        folder_id,
        expected_generation,
        owner_fusionauth_user_id,
    )
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(row)
}

/// Mark a folder absent from the latest traversal without deleting local mail.
#[tracing::instrument(err, skip(pool))]
pub async fn mark_microsoft_folder_deleted_for_owner(
    pool: &PgPool,
    owner_fusionauth_user_id: &str,
    link_id: Uuid,
    folder_id: &str,
) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    if !crate::microsoft_mailbox::lock_active_microsoft_mailbox_for_owner(
        &mut tx,
        owner_fusionauth_user_id,
        link_id,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(false);
    }

    let changed = sqlx::query_scalar!(
        r#"
        UPDATE email_microsoft_folder_sync f
        SET is_deleted = true,
            delta_cursor = NULL,
            cursor_generation = f.cursor_generation + 1,
            updated_at = now()
        FROM email_microsoft_mailboxes m, email_links l
        WHERE f.link_id = $1
          AND f.folder_id = $2
          AND l.fusionauth_user_id = $3
          AND m.link_id = f.link_id
          AND l.id = f.link_id
          AND l.provider = 'MICROSOFT'::email_user_provider_enum
          AND l.is_sync_active = true
          AND m.disconnected_at IS NULL
          AND f.is_deleted = false
        RETURNING f.link_id
        "#,
        link_id,
        folder_id,
        owner_fusionauth_user_id,
    )
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(changed.is_some())
}
