use super::*;
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::{email::EmailStr, user_id::MacroUserIdStr};
use models_email::email::service::{
    link::{Link, UserProvider},
    microsoft::NewMicrosoftMailbox,
};
use sqlx::{Pool, Postgres};

const OWNER: &str = "owner-a";
const EMAIL: &str = "work@example.com";

async fn provision(pool: &Pool<Postgres>) -> anyhow::Result<Uuid> {
    let link = Link {
        id: macro_uuid::generate_uuid_v7(),
        macro_id: MacroUserIdStr::try_from(format!("macro|{EMAIL}"))?,
        fusionauth_user_id: OWNER.to_owned(),
        email_address: EmailStr::try_from(EMAIL.to_owned())?,
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
        OWNER,
        NewMicrosoftMailbox {
            link_id: link.id,
            tenant_id: "tenant-a".to_owned(),
            mailbox_id: "mailbox-a".to_owned(),
            user_principal_name: EMAIL.to_owned(),
        },
    )
    .await?
    .expect("mailbox provisioned");
    Ok(link.id)
}

fn inbox(link_id: Uuid, display_name: &str) -> DiscoveredMicrosoftFolder {
    DiscoveredMicrosoftFolder {
        link_id,
        folder_id: "folder-inbox".to_owned(),
        parent_folder_id: None,
        display_name: display_name.to_owned(),
        well_known_name: Some("inbox".to_owned()),
    }
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn folder_upsert_is_idempotent_and_owner_scoped(pool: Pool<Postgres>) -> anyhow::Result<()> {
    let link_id = provision(&pool).await?;
    assert!(
        upsert_microsoft_folder_for_owner(&pool, "owner-b", inbox(link_id, "Inbox"))
            .await?
            .is_none()
    );

    let first = upsert_microsoft_folder_for_owner(&pool, OWNER, inbox(link_id, "Inbox"))
        .await?
        .expect("folder inserted");
    let second = upsert_microsoft_folder_for_owner(&pool, OWNER, inbox(link_id, "Inbox renamed"))
        .await?
        .expect("folder updated");

    assert_eq!(first.folder_id, second.folder_id);
    assert_eq!(second.display_name, "Inbox renamed");
    assert_eq!(second.cursor_generation, 0);
    assert_eq!(
        list_microsoft_folders_for_owner(&pool, OWNER, link_id)
            .await?
            .len(),
        1
    );
    assert!(
        list_microsoft_folders_for_owner(&pool, "owner-b", link_id)
            .await?
            .is_empty()
    );
    Ok(())
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn cursor_compare_and_swap_is_restart_safe(pool: Pool<Postgres>) -> anyhow::Result<()> {
    let link_id = provision(&pool).await?;
    let folder = upsert_microsoft_folder_for_owner(&pool, OWNER, inbox(link_id, "Inbox"))
        .await?
        .expect("folder inserted");

    let committed = commit_microsoft_folder_cursor_for_owner(
        &pool,
        OWNER,
        link_id,
        &folder.folder_id,
        folder.cursor_generation,
        "delta-1",
        true,
    )
    .await?
    .expect("first worker commits");
    assert_eq!(committed.cursor_generation, 1);
    assert_eq!(committed.delta_cursor.as_deref(), Some("delta-1"));

    assert!(
        commit_microsoft_folder_cursor_for_owner(
            &pool,
            OWNER,
            link_id,
            &folder.folder_id,
            folder.cursor_generation,
            "stale-worker-cursor",
            true,
        )
        .await?
        .is_none()
    );

    let after_restart = list_microsoft_folders_for_owner(&pool, OWNER, link_id)
        .await?
        .pop()
        .expect("folder remains");
    assert_eq!(after_restart.delta_cursor.as_deref(), Some("delta-1"));

    let invalidated = invalidate_microsoft_folder_cursor_for_owner(
        &pool,
        OWNER,
        link_id,
        &folder.folder_id,
        after_restart.cursor_generation,
    )
    .await?
    .expect("expired cursor invalidated");
    assert_eq!(invalidated.cursor_generation, 2);
    assert!(invalidated.delta_cursor.is_none());
    assert!(!invalidated.initial_sync_complete);
    assert!(invalidated.cursor_invalidated_at.is_some());
    Ok(())
}
