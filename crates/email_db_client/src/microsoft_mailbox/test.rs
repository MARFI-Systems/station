use super::*;
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::{email::EmailStr, user_id::MacroUserIdStr};
use models_email::email::service::link::{Link, UserProvider};
use sqlx::{Pool, Postgres};

fn link(owner: &str, email: &str, provider: UserProvider) -> Link {
    Link {
        id: macro_uuid::generate_uuid_v7(),
        macro_id: MacroUserIdStr::try_from(format!("macro|{email}")).unwrap(),
        fusionauth_user_id: owner.to_owned(),
        email_address: EmailStr::try_from(email.to_owned()).unwrap(),
        provider,
        is_sync_active: true,
        is_primary: true,
        needs_reauth: false,
        last_sync_error_at: None,
        created_at: Default::default(),
        updated_at: Default::default(),
    }
}

async fn insert_link(pool: &Pool<Postgres>, link: Link) -> anyhow::Result<Link> {
    let mut conn = pool.acquire().await?;
    crate::links::insert::upsert_link(&mut conn, link).await
}

fn mailbox(link_id: Uuid, email: &str) -> NewMicrosoftMailbox {
    NewMicrosoftMailbox {
        link_id,
        tenant_id: "tenant-a".to_owned(),
        mailbox_id: format!("mailbox-{email}"),
        user_principal_name: email.to_owned(),
    }
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn microsoft_mailbox_is_owner_scoped_and_gmail_is_preserved(
    pool: Pool<Postgres>,
) -> anyhow::Result<()> {
    let owner = "owner-a";
    let gmail = insert_link(&pool, link(owner, "gmail@example.com", UserProvider::Gmail)).await?;
    let microsoft = insert_link(
        &pool,
        link(owner, "work@example.com", UserProvider::Microsoft),
    )
    .await?;

    assert!(
        upsert_microsoft_mailbox_for_owner(
            &pool,
            "owner-b",
            mailbox(microsoft.id, "work@example.com")
        )
        .await?
        .is_none()
    );

    let inserted =
        upsert_microsoft_mailbox_for_owner(&pool, owner, mailbox(microsoft.id, "WORK@EXAMPLE.COM"))
            .await?
            .expect("owner can provision their mailbox");
    assert_eq!(inserted.user_principal_name, "work@example.com");

    assert!(
        fetch_active_microsoft_mailbox_for_owner(&pool, "owner-b", microsoft.id)
            .await?
            .is_none()
    );
    assert!(
        fetch_active_microsoft_mailbox_for_owner(&pool, owner, microsoft.id)
            .await?
            .is_some()
    );

    let gmail_after = crate::links::get::fetch_link_by_id(&pool, gmail.id)
        .await?
        .expect("Gmail link remains present");
    assert_eq!(gmail_after.provider, UserProvider::Gmail);
    assert!(gmail_after.is_sync_active);
    Ok(())
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn disconnect_only_blocks_the_target_owners_mailbox(
    pool: Pool<Postgres>,
) -> anyhow::Result<()> {
    let owner_a = "owner-a";
    let owner_b = "owner-b";
    let first = insert_link(
        &pool,
        link(owner_a, "first@example.com", UserProvider::Microsoft),
    )
    .await?;
    let second = insert_link(
        &pool,
        link(owner_b, "second@example.com", UserProvider::Microsoft),
    )
    .await?;
    upsert_microsoft_mailbox_for_owner(&pool, owner_a, mailbox(first.id, "first@example.com"))
        .await?;
    upsert_microsoft_mailbox_for_owner(&pool, owner_b, mailbox(second.id, "second@example.com"))
        .await?;

    assert!(!disconnect_microsoft_mailbox_for_owner(&pool, owner_b, first.id).await?);
    assert!(
        fetch_active_microsoft_mailbox_for_owner(&pool, owner_a, first.id)
            .await?
            .is_some()
    );

    assert!(disconnect_microsoft_mailbox_for_owner(&pool, owner_a, first.id).await?);
    assert!(
        fetch_active_microsoft_mailbox_for_owner(&pool, owner_a, first.id)
            .await?
            .is_none()
    );
    assert!(
        fetch_active_microsoft_mailbox_for_owner(&pool, owner_b, second.id)
            .await?
            .is_some()
    );
    Ok(())
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn provisioning_is_atomic_and_idempotent(pool: Pool<Postgres>) -> anyhow::Result<()> {
    let request = link("owner-a", "work@example.com", UserProvider::Microsoft);
    let link_id = request.id;
    let (first_link, first_mailbox) =
        provision_microsoft_mailbox(&pool, request.clone(), mailbox(link_id, "work@example.com"))
            .await?;
    let (second_link, second_mailbox) =
        provision_microsoft_mailbox(&pool, request, mailbox(link_id, "work@example.com")).await?;

    assert_eq!(first_link.id, second_link.id);
    assert_eq!(first_mailbox.link_id, second_mailbox.link_id);
    let count = sqlx::query_scalar!(
        r#"SELECT COUNT(*) as "count!" FROM email_links
           WHERE fusionauth_user_id = $1
             AND provider = 'MICROSOFT'::email_user_provider_enum"#,
        "owner-a",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(count, 1);
    Ok(())
}
