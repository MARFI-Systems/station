use super::*;
use chrono::Utc;
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::{email::EmailStr, user_id::MacroUserIdStr};
use models_email::email::service::{
    attachment::Attachment,
    label::{Label, LabelListVisibility, LabelType, MessageListVisibility},
    link::{Link, UserProvider},
    microsoft::{DiscoveredMicrosoftFolder, NewMicrosoftMailbox},
};
use sqlx::{Pool, Postgres};

const OWNER_A: &str = "owner-a";
const OWNER_B: &str = "owner-b";
const EMAIL_A: &str = "owner-a@example.com";
const FOLDER_A: &str = "folder-a";
const FOLDER_B: &str = "folder-b";

async fn provision(pool: &Pool<Postgres>) -> anyhow::Result<Uuid> {
    let link = Link {
        id: macro_uuid::generate_uuid_v7(),
        macro_id: MacroUserIdStr::try_from(format!("macro|{EMAIL_A}"))?,
        fusionauth_user_id: OWNER_A.to_owned(),
        email_address: EmailStr::try_from(EMAIL_A.to_owned())?,
        provider: UserProvider::Microsoft,
        is_sync_active: true,
        is_primary: true,
        needs_reauth: false,
        last_sync_error_at: None,
        created_at: Default::default(),
        updated_at: Default::default(),
    };
    let mut conn = pool.acquire().await?;
    let link = crate::links::insert::upsert_link(&mut conn, link).await?;
    crate::microsoft_mailbox::upsert_microsoft_mailbox_for_owner(
        pool,
        OWNER_A,
        NewMicrosoftMailbox {
            link_id: link.id,
            tenant_id: "tenant-a".to_owned(),
            mailbox_id: "mailbox-a".to_owned(),
            user_principal_name: EMAIL_A.to_owned(),
        },
    )
    .await?
    .expect("mailbox provisioned");
    Ok(link.id)
}

fn folder(link_id: Uuid, folder_id: &str) -> DiscoveredMicrosoftFolder {
    DiscoveredMicrosoftFolder {
        link_id,
        folder_id: folder_id.to_owned(),
        parent_folder_id: None,
        display_name: folder_id.to_owned(),
        well_known_name: None,
    }
}

async fn insert_folder(
    pool: &Pool<Postgres>,
    link_id: Uuid,
    folder_id: &str,
) -> anyhow::Result<()> {
    crate::microsoft_folder_sync::upsert_microsoft_folder_for_owner(
        pool,
        OWNER_A,
        folder(link_id, folder_id),
    )
    .await?
    .expect("folder inserted");
    Ok(())
}

async fn insert_label(pool: &Pool<Postgres>, link_id: Uuid) -> anyhow::Result<()> {
    crate::labels::insert::insert_or_update_labels(
        pool,
        vec![Label {
            id: None,
            link_id,
            provider_label_id: "microsoft-folder:folder-a".to_owned(),
            name: Some("Folder A".to_owned()),
            created_at: Utc::now(),
            message_list_visibility: Some(MessageListVisibility::Show),
            label_list_visibility: Some(LabelListVisibility::LabelShow),
            type_: Some(LabelType::User),
        }],
    )
    .await
}

fn projected_message(link_id: Uuid, provider_message_id: &str) -> Message {
    let now = Utc::now();
    Message {
        db_id: macro_uuid::generate_uuid_v7(),
        provider_id: Some(provider_message_id.to_owned()),
        thread_db_id: Uuid::nil(),
        provider_thread_id: Some("provider-thread-1".to_owned()),
        replying_to_id: None,
        global_id: Some(format!("<{provider_message_id}@example.com>")),
        link_id,
        subject: Some("Native projection".to_owned()),
        snippet: Some("snippet".to_owned()),
        provider_history_id: Some("change-1".to_owned()),
        internal_date_ts: Some(now),
        sent_at: Some(now),
        size_estimate: Some(123),
        is_read: false,
        is_starred: false,
        is_sent: false,
        is_draft: false,
        scheduled_send_time: None,
        has_attachments: true,
        from: None,
        to: vec![],
        cc: vec![],
        bcc: vec![],
        labels: vec![Label {
            id: None,
            link_id,
            provider_label_id: "microsoft-folder:folder-a".to_owned(),
            name: Some("Folder A".to_owned()),
            created_at: now,
            message_list_visibility: Some(MessageListVisibility::Show),
            label_list_visibility: Some(LabelListVisibility::LabelShow),
            type_: Some(LabelType::User),
        }],
        body_text: Some("body".to_owned()),
        body_html_sanitized: None,
        body_macro: None,
        attachments: vec![Attachment {
            db_id: macro_uuid::generate_uuid_v7(),
            provider_id: Some("attachment-1".to_owned()),
            data_url: None,
            filename: Some("native.txt".to_owned()),
            mime_type: Some("text/plain".to_owned()),
            size_bytes: Some(4),
            sfs_id: None,
            content_id: Some("content-1".to_owned()),
        }],
        attachments_draft: vec![],
        attachments_forwarded: vec![],
        headers_json: None,
        created_at: now,
        updated_at: now,
    }
}

async fn projection_count(
    pool: &Pool<Postgres>,
    table: &str,
    link_id: Uuid,
) -> anyhow::Result<i64> {
    let sql = format!("SELECT count(*) FROM {table} WHERE link_id = $1");
    Ok(sqlx::query_scalar(&sql)
        .bind(link_id)
        .fetch_one(pool)
        .await?)
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn native_projection_round_trip_is_idempotent(pool: Pool<Postgres>) -> anyhow::Result<()> {
    let link_id = provision(&pool).await?;
    insert_folder(&pool, link_id, FOLDER_A).await?;
    insert_label(&pool, link_id).await?;

    let mut first = projected_message(link_id, "message-1");
    assert!(upsert_projected_message_for_owner(&pool, OWNER_A, FOLDER_A, &mut first).await?);
    let mut replay = projected_message(link_id, "message-1");
    replay.subject = Some("Native projection updated".to_owned());
    assert!(upsert_projected_message_for_owner(&pool, OWNER_A, FOLDER_A, &mut replay).await?);

    assert_eq!(projection_count(&pool, "email_threads", link_id).await?, 1);
    assert_eq!(projection_count(&pool, "email_messages", link_id).await?, 1);
    assert_eq!(
        projection_count(&pool, "email_microsoft_message_state", link_id).await?,
        1
    );
    let attachment_count: i64 = sqlx::query_scalar(
        r#"SELECT count(*) FROM email_attachments a
           JOIN email_messages m ON m.id = a.message_id WHERE m.link_id = $1"#,
    )
    .bind(link_id)
    .fetch_one(&pool)
    .await?;
    let label_count: i64 = sqlx::query_scalar(
        r#"SELECT count(*) FROM email_message_labels ml
           JOIN email_messages m ON m.id = ml.message_id WHERE m.link_id = $1"#,
    )
    .bind(link_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(attachment_count, 1);
    assert_eq!(label_count, 1);

    let subject: Option<String> =
        sqlx::query_scalar("SELECT subject FROM email_messages WHERE link_id = $1")
            .bind(link_id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(subject.as_deref(), Some("Native projection updated"));
    Ok(())
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn destination_upsert_wins_over_source_tombstone(pool: Pool<Postgres>) -> anyhow::Result<()> {
    let link_id = provision(&pool).await?;
    insert_folder(&pool, link_id, FOLDER_A).await?;
    insert_folder(&pool, link_id, FOLDER_B).await?;
    insert_label(&pool, link_id).await?;

    let mut source = projected_message(link_id, "message-move");
    assert!(upsert_projected_message_for_owner(&pool, OWNER_A, FOLDER_A, &mut source).await?);
    let mut destination = projected_message(link_id, "message-move");
    assert!(upsert_projected_message_for_owner(&pool, OWNER_A, FOLDER_B, &mut destination).await?);

    assert!(
        !remove_projected_message_from_folder_for_owner(
            &pool,
            OWNER_A,
            link_id,
            FOLDER_A,
            "message-move",
        )
        .await?
    );
    let state_folder: String = sqlx::query_scalar(
        "SELECT folder_id FROM email_microsoft_message_state WHERE link_id = $1 AND provider_message_id = $2",
    )
    .bind(link_id)
    .bind("message-move")
    .fetch_one(&pool)
    .await?;
    assert_eq!(state_folder, FOLDER_B);
    assert_eq!(projection_count(&pool, "email_messages", link_id).await?, 1);
    Ok(())
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn completed_discovery_tombstones_only_unseen_folders(
    pool: Pool<Postgres>,
) -> anyhow::Result<()> {
    let link_id = provision(&pool).await?;
    insert_folder(&pool, link_id, FOLDER_A).await?;
    insert_folder(&pool, link_id, FOLDER_B).await?;

    assert!(commit_folder_walk_step_for_owner(
        &pool,
        OWNER_A,
        link_id,
        0,
        None,
        &[FOLDER_A.to_owned()],
    )
    .await?);

    let rows: Vec<(String, bool, i64)> = sqlx::query_as(
        r#"SELECT folder_id, is_deleted, cursor_generation
           FROM email_microsoft_folder_sync
           WHERE link_id = $1 ORDER BY folder_id"#,
    )
    .bind(link_id)
    .fetch_all(&pool)
    .await?;
    assert_eq!(
        rows,
        vec![
            (FOLDER_A.to_owned(), false, 0),
            (FOLDER_B.to_owned(), true, 1)
        ]
    );
    let walk: (i64, Option<serde_json::Value>, Vec<String>) = sqlx::query_as(
        r#"SELECT folder_walk_generation, folder_walk_cursor, folder_walk_seen_ids
           FROM email_microsoft_mailboxes WHERE link_id = $1"#,
    )
    .bind(link_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(walk.0, 1);
    assert!(walk.1.is_none());
    assert!(walk.2.is_empty());
    Ok(())
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn owner_boundary_blocks_projection_mutations(pool: Pool<Postgres>) -> anyhow::Result<()> {
    let link_id = provision(&pool).await?;
    insert_folder(&pool, link_id, FOLDER_A).await?;
    insert_label(&pool, link_id).await?;
    let mut owned = projected_message(link_id, "message-owner");
    assert!(upsert_projected_message_for_owner(&pool, OWNER_A, FOLDER_A, &mut owned).await?);

    let mut cross_owner = projected_message(link_id, "message-cross-owner");
    assert!(
        !upsert_projected_message_for_owner(&pool, OWNER_B, FOLDER_A, &mut cross_owner,).await?
    );
    assert!(
        !remove_projected_message_from_folder_for_owner(
            &pool,
            OWNER_B,
            link_id,
            FOLDER_A,
            "message-owner",
        )
        .await?
    );
    assert_eq!(
        clear_projected_folder_for_owner(&pool, OWNER_B, link_id, FOLDER_A).await?,
        (0, false)
    );
    assert_eq!(projection_count(&pool, "email_messages", link_id).await?, 1);
    Ok(())
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn rebuild_clear_is_bounded_and_eventually_empty(pool: Pool<Postgres>) -> anyhow::Result<()> {
    let link_id = provision(&pool).await?;
    insert_folder(&pool, link_id, FOLDER_A).await?;
    let thread_id = macro_uuid::generate_uuid_v7();
    sqlx::query("INSERT INTO email_threads (id, provider_id, link_id) VALUES ($1, $2, $3)")
        .bind(thread_id)
        .bind("bulk-thread")
        .bind(link_id)
        .execute(&pool)
        .await?;

    let message_ids: Vec<Uuid> = (0..501).map(|_| macro_uuid::generate_uuid_v7()).collect();
    let provider_ids: Vec<String> = (0..501).map(|i| format!("bulk-message-{i}")).collect();
    sqlx::query(
        r#"INSERT INTO email_messages
           (id, provider_id, thread_id, provider_thread_id, link_id, subject)
           SELECT id, provider_id, $3, 'bulk-thread', $4, 'bulk'
           FROM unnest($1::uuid[], $2::text[]) AS input(id, provider_id)"#,
    )
    .bind(&message_ids)
    .bind(&provider_ids)
    .bind(thread_id)
    .bind(link_id)
    .execute(&pool)
    .await?;
    sqlx::query(
        r#"INSERT INTO email_microsoft_message_state
           (link_id, provider_message_id, message_id, folder_id, provider_thread_id)
           SELECT $1, provider_id, id, $4, 'bulk-thread'
           FROM unnest($2::uuid[], $3::text[]) AS input(id, provider_id)"#,
    )
    .bind(link_id)
    .bind(&message_ids)
    .bind(&provider_ids)
    .bind(FOLDER_A)
    .execute(&pool)
    .await?;

    assert_eq!(
        clear_projected_folder_for_owner(&pool, OWNER_A, link_id, FOLDER_A).await?,
        (500, true)
    );
    assert_eq!(
        clear_projected_folder_for_owner(&pool, OWNER_A, link_id, FOLDER_A).await?,
        (1, false)
    );
    assert_eq!(projection_count(&pool, "email_messages", link_id).await?, 0);
    assert_eq!(projection_count(&pool, "email_threads", link_id).await?, 0);
    assert_eq!(
        projection_count(&pool, "email_microsoft_message_state", link_id).await?,
        0
    );
    Ok(())
}

async fn assert_mailbox_rejects_mutation(
    pool: &Pool<Postgres>,
    link_id: Uuid,
    provider_message_id: &str,
) -> anyhow::Result<()> {
    let mut message = projected_message(link_id, "blocked-upsert");
    assert!(!upsert_projected_message_for_owner(pool, OWNER_A, FOLDER_A, &mut message).await?);
    assert!(
        !remove_projected_message_from_folder_for_owner(
            pool,
            OWNER_A,
            link_id,
            FOLDER_A,
            provider_message_id,
        )
        .await?
    );
    assert_eq!(
        clear_projected_folder_for_owner(pool, OWNER_A, link_id, FOLDER_A).await?,
        (0, false)
    );
    assert!(
        crate::microsoft_folder_sync::upsert_microsoft_folder_for_owner(
            pool,
            OWNER_A,
            folder(link_id, "blocked-folder"),
        )
        .await?
        .is_none()
    );
    assert!(
        crate::microsoft_folder_sync::commit_microsoft_folder_cursor_for_owner(
            pool,
            OWNER_A,
            link_id,
            FOLDER_A,
            0,
            "delta-blocked",
            true,
        )
        .await?
        .is_none()
    );
    assert!(
        crate::microsoft_folder_sync::invalidate_microsoft_folder_cursor_for_owner(
            pool, OWNER_A, link_id, FOLDER_A, 0,
        )
        .await?
        .is_none()
    );
    assert!(
        !crate::microsoft_folder_sync::mark_microsoft_folder_deleted_for_owner(
            pool, OWNER_A, link_id, FOLDER_A,
        )
        .await?
    );
    assert!(
        !commit_folder_walk_step_for_owner(
            pool,
            OWNER_A,
            link_id,
            0,
            Some(serde_json::json!({"cursor": "next"})),
            &[FOLDER_A.to_owned()],
        )
        .await?
    );
    Ok(())
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn inactive_and_disconnected_mailboxes_cannot_mutate_or_advance_discovery(
    pool: Pool<Postgres>,
) -> anyhow::Result<()> {
    let link_id = provision(&pool).await?;
    insert_folder(&pool, link_id, FOLDER_A).await?;
    insert_label(&pool, link_id).await?;
    let mut existing = projected_message(link_id, "existing-message");
    assert!(upsert_projected_message_for_owner(&pool, OWNER_A, FOLDER_A, &mut existing).await?);

    sqlx::query("UPDATE email_links SET is_sync_active = false WHERE id = $1")
        .bind(link_id)
        .execute(&pool)
        .await?;
    assert_mailbox_rejects_mutation(&pool, link_id, "existing-message").await?;
    assert_eq!(projection_count(&pool, "email_messages", link_id).await?, 1);

    sqlx::query("UPDATE email_links SET is_sync_active = true WHERE id = $1")
        .bind(link_id)
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE email_microsoft_mailboxes SET disconnected_at = now() WHERE link_id = $1")
        .bind(link_id)
        .execute(&pool)
        .await?;
    assert_mailbox_rejects_mutation(&pool, link_id, "existing-message").await?;
    assert_eq!(projection_count(&pool, "email_messages", link_id).await?, 1);
    Ok(())
}
