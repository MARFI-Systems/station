use super::*;
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use sha2::Digest;
use sqlx::{Pool, Postgres};

const OWNER_ID: &str = "fusionauth-user-1";

fn encrypted_grant(seed: u8) -> EncryptedMicrosoftOAuthGrant {
    EncryptedMicrosoftOAuthGrant::new(
        vec![seed, 0, 255, seed.wrapping_add(1)],
        vec![seed.wrapping_add(2), 0, 128],
        vec![seed; 12],
        i32::from(seed) + 1,
        format!("arn:aws:kms:us-east-1:123456789012:key/{seed}"),
    )
}

async fn upsert_test_mailbox_grant(
    pool: &Pool<Postgres>,
    owner: &str,
    email: &str,
    grant: &EncryptedMicrosoftOAuthGrant,
) -> anyhow::Result<StoredMicrosoftOAuthGrant> {
    upsert_microsoft_mailbox_grant(
        pool,
        owner,
        email,
        "tenant-id",
        &format!("object-{owner}"),
        grant,
    )
    .await
}

fn assert_envelopes_equal(
    actual: &EncryptedMicrosoftOAuthGrant,
    expected: &EncryptedMicrosoftOAuthGrant,
) {
    assert_eq!(
        actual.refresh_token_ciphertext(),
        expected.refresh_token_ciphertext()
    );
    assert_eq!(actual.encrypted_data_key(), expected.encrypted_data_key());
    assert_eq!(actual.nonce(), expected.nonce());
    assert_eq!(actual.encryption_version(), expected.encryption_version());
    assert_eq!(actual.kms_key_id(), expected.kms_key_id());
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn normalizes_email_addresses_for_upsert_and_fetch(
    pool: Pool<Postgres>,
) -> anyhow::Result<()> {
    let envelope = encrypted_grant(1);

    let inserted =
        upsert_test_mailbox_grant(&pool, OWNER_ID, "Mailbox@Example.COM", &envelope).await?;
    let fetched = get_microsoft_oauth_grant(&pool, OWNER_ID, "MAILBOX@EXAMPLE.COM")
        .await?
        .expect("normalized grant should exist");

    assert_eq!(inserted.email_address(), "mailbox@example.com");
    assert_eq!(fetched.email_address(), "mailbox@example.com");

    let uppercase_update = sqlx::query!(
        r#"
            UPDATE microsoft_oauth_grants
            SET email_address = 'Mailbox@Example.COM'
            WHERE fusionauth_user_id = $1
              AND email_address = 'mailbox@example.com'
        "#,
        OWNER_ID,
    )
    .execute(&pool)
    .await;
    assert!(
        uppercase_update.is_err(),
        "the database must reject non-normalized mailbox keys"
    );

    Ok(())
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn first_insert_stores_encrypted_envelope_and_metadata(
    pool: Pool<Postgres>,
) -> anyhow::Result<()> {
    let before_insert = Utc::now();
    let envelope = encrypted_grant(2);

    let inserted =
        upsert_test_mailbox_grant(&pool, OWNER_ID, "mailbox@example.com", &envelope).await?;

    assert_eq!(inserted.fusionauth_user_id(), OWNER_ID);
    assert_envelopes_equal(inserted.encrypted_grant(), &envelope);
    assert!(inserted.created_at() >= before_insert);
    assert_eq!(inserted.updated_at(), inserted.created_at());
    assert_eq!(inserted.last_refreshed_at(), inserted.created_at());

    Ok(())
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn reconnect_atomically_replaces_the_complete_envelope(
    pool: Pool<Postgres>,
) -> anyhow::Result<()> {
    let initial = encrypted_grant(3);
    let replacement = encrypted_grant(20);
    let first = upsert_test_mailbox_grant(&pool, OWNER_ID, "mailbox@example.com", &initial).await?;

    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    let reconnected =
        upsert_test_mailbox_grant(&pool, OWNER_ID, "mailbox@example.com", &replacement).await?;

    assert_eq!(reconnected.created_at(), first.created_at());
    assert!(reconnected.updated_at() > first.updated_at());
    assert!(reconnected.last_refreshed_at() > first.last_refreshed_at());
    assert_envelopes_equal(reconnected.encrypted_grant(), &replacement);
    assert_ne!(
        reconnected.encrypted_grant().refresh_token_ciphertext(),
        initial.refresh_token_ciphertext()
    );
    assert_ne!(
        reconnected.encrypted_grant().encrypted_data_key(),
        initial.encrypted_data_key()
    );
    assert_ne!(reconnected.encrypted_grant().nonce(), initial.nonce());
    assert_ne!(
        reconnected.encrypted_grant().encryption_version(),
        initial.encryption_version()
    );
    assert_ne!(
        reconnected.encrypted_grant().kms_key_id(),
        initial.kms_key_id()
    );

    Ok(())
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn owner_and_normalized_mailbox_form_a_unique_key(
    pool: Pool<Postgres>,
) -> anyhow::Result<()> {
    let owner_one_envelope = encrypted_grant(4);
    let owner_two_envelope = encrypted_grant(5);

    upsert_test_mailbox_grant(
        &pool,
        "fusionauth-user-1",
        "shared@example.com",
        &owner_one_envelope,
    )
    .await?;
    upsert_test_mailbox_grant(
        &pool,
        "fusionauth-user-2",
        "shared@example.com",
        &owner_two_envelope,
    )
    .await?;

    let duplicate_result = sqlx::query!(
        r#"
            INSERT INTO microsoft_oauth_grants (
                fusionauth_user_id,
                email_address,
                refresh_token_ciphertext,
                encrypted_data_key,
                nonce,
                encryption_version,
                kms_key_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
        "fusionauth-user-1",
        "shared@example.com",
        owner_one_envelope.refresh_token_ciphertext,
        owner_one_envelope.encrypted_data_key,
        owner_one_envelope.nonce,
        owner_one_envelope.encryption_version,
        owner_one_envelope.kms_key_id,
    )
    .execute(&pool)
    .await;

    assert!(duplicate_result.is_err(), "duplicate owner key must fail");
    let grant_count = sqlx::query_scalar!(
        r#"
            SELECT COUNT(*) AS "count!"
            FROM microsoft_oauth_grants
            WHERE email_address = 'shared@example.com'
        "#
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(grant_count, 2, "ownership must be scoped to the user");

    Ok(())
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn opaque_binary_values_round_trip_without_interpretation(
    pool: Pool<Postgres>,
) -> anyhow::Result<()> {
    let envelope = EncryptedMicrosoftOAuthGrant::new(
        vec![0, 255, 0, 17, 128, 42],
        vec![255, 254, 0, 1, 2],
        vec![0, 1, 2, 3, 4, 5, 255, 128, 10, 11, 12, 13],
        i32::MAX,
        "alias/microsoft-oauth-grants".to_string(),
    );

    upsert_test_mailbox_grant(&pool, OWNER_ID, "binary@example.com", &envelope).await?;
    let fetched = get_microsoft_oauth_grant(&pool, OWNER_ID, "binary@example.com")
        .await?
        .expect("grant should exist");

    assert_envelopes_equal(fetched.encrypted_grant(), &envelope);

    Ok(())
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn schema_has_no_plaintext_secret_columns(pool: Pool<Postgres>) -> anyhow::Result<()> {
    let columns = sqlx::query_scalar!(
        r#"
            SELECT column_name AS "column_name!"
            FROM information_schema.columns
            WHERE table_schema = 'public'
              AND table_name = 'microsoft_oauth_grants'
            ORDER BY ordinal_position
        "#
    )
    .fetch_all(&pool)
    .await?;

    assert_eq!(
        columns,
        vec![
            "fusionauth_user_id",
            "email_address",
            "refresh_token_ciphertext",
            "encrypted_data_key",
            "nonce",
            "encryption_version",
            "kms_key_id",
            "created_at",
            "updated_at",
            "last_refreshed_at",
            "grant_purpose",
            "grant_schema_version",
            "microsoft_tenant_id",
            "microsoft_object_id",
            "disconnected_at",
        ]
    );
    assert!(!columns.iter().any(|column| {
        matches!(
            column.as_str(),
            "refresh_token" | "plaintext_refresh_token" | "data_key"
        )
    }));

    Ok(())
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn stale_refresh_cannot_overwrite_or_revoke_a_rotated_grant(
    pool: Pool<Postgres>,
) -> anyhow::Result<()> {
    let initial = encrypted_grant(30);
    let rotated = encrypted_grant(31);
    upsert_test_mailbox_grant(&pool, OWNER_ID, "owner@example.com", &initial).await?;

    assert!(
        replace_microsoft_oauth_grant_if_current(
            &pool,
            OWNER_ID,
            "owner@example.com",
            &initial,
            &rotated,
        )
        .await?
    );
    assert!(
        !replace_microsoft_oauth_grant_if_current(
            &pool,
            OWNER_ID,
            "owner@example.com",
            &initial,
            &encrypted_grant(32),
        )
        .await?
    );
    assert!(
        !delete_microsoft_oauth_grant_if_current(&pool, OWNER_ID, "owner@example.com", &initial,)
            .await?
    );

    let fetched = get_microsoft_oauth_grant(&pool, OWNER_ID, "owner@example.com")
        .await?
        .expect("rotated grant remains");
    assert_envelopes_equal(fetched.encrypted_grant(), &rotated);
    Ok(())
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn oauth_flow_is_owner_bound_tamper_evident_and_single_use(
    pool: Pool<Postgres>,
) -> anyhow::Result<()> {
    let flow_id = uuid::Uuid::now_v7();
    let state_hash = sha2::Sha256::digest(b"original-state").to_vec();
    create_microsoft_mailbox_oauth_flow(
        &pool,
        flow_id,
        OWNER_ID,
        &state_hash,
        &encrypted_grant(40),
        Utc::now() + chrono::Duration::minutes(5),
    )
    .await?;
    assert!(
        consume_microsoft_mailbox_oauth_flow(&pool, flow_id, "another-owner", &state_hash)
            .await?
            .is_none()
    );
    assert!(
        consume_microsoft_mailbox_oauth_flow(
            &pool,
            flow_id,
            OWNER_ID,
            &sha2::Sha256::digest(b"tampered-state")
        )
        .await?
        .is_none()
    );
    assert!(
        consume_microsoft_mailbox_oauth_flow(&pool, flow_id, OWNER_ID, &state_hash)
            .await?
            .is_some()
    );
    assert!(
        consume_microsoft_mailbox_oauth_flow(&pool, flow_id, OWNER_ID, &state_hash)
            .await?
            .is_none()
    );
    Ok(())
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn oauth_flow_rejects_expiry_beyond_server_window(
    pool: Pool<Postgres>,
) -> anyhow::Result<()> {
    let result = create_microsoft_mailbox_oauth_flow(
        &pool,
        uuid::Uuid::now_v7(),
        OWNER_ID,
        &sha2::Sha256::digest(b"state"),
        &encrypted_grant(41),
        Utc::now() + chrono::Duration::minutes(11),
    )
    .await;
    assert!(result.is_err());
    Ok(())
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn legacy_identity_grants_fail_closed(pool: Pool<Postgres>) -> anyhow::Result<()> {
    upsert_microsoft_oauth_grant(&pool, OWNER_ID, "owner@example.com", &encrypted_grant(42))
        .await?;
    assert!(
        get_active_microsoft_mailbox_grant(&pool, OWNER_ID)
            .await?
            .is_none()
    );
    Ok(())
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn disconnect_prevents_refresh_cas_from_resurrecting_grant(
    pool: Pool<Postgres>,
) -> anyhow::Result<()> {
    upsert_test_mailbox_grant(&pool, OWNER_ID, "owner@example.com", &encrypted_grant(43)).await?;
    let current = get_active_microsoft_mailbox_grant(&pool, OWNER_ID)
        .await?
        .unwrap();
    assert!(disconnect_microsoft_mailbox_grant(&pool, OWNER_ID).await?);
    assert!(
        !replace_microsoft_oauth_grant_if_current(
            &pool,
            OWNER_ID,
            "owner@example.com",
            current.encrypted_grant(),
            &encrypted_grant(44)
        )
        .await?
    );
    assert!(
        get_active_microsoft_mailbox_grant(&pool, OWNER_ID)
            .await?
            .is_none()
    );
    Ok(())
}

async fn create_and_consume_flow(pool: &Pool<Postgres>, seed: u8) -> anyhow::Result<uuid::Uuid> {
    let flow_id = uuid::Uuid::now_v7();
    let state_hash = sha2::Sha256::digest([seed]).to_vec();
    create_microsoft_mailbox_oauth_flow(
        pool,
        flow_id,
        OWNER_ID,
        &state_hash,
        &encrypted_grant(seed),
        Utc::now() + chrono::Duration::minutes(5),
    )
    .await?;
    consume_microsoft_mailbox_oauth_flow(pool, flow_id, OWNER_ID, &state_hash)
        .await?
        .expect("flow should consume once");
    Ok(flow_id)
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn disconnect_after_consume_prevents_callback_finalization(
    pool: Pool<Postgres>,
) -> anyhow::Result<()> {
    let flow_id = create_and_consume_flow(&pool, 50).await?;
    assert!(!disconnect_microsoft_mailbox_grant(&pool, OWNER_ID).await?);
    let finalized = finalize_microsoft_mailbox_oauth_flow(
        &pool,
        flow_id,
        OWNER_ID,
        "owner@example.com",
        "tenant-id",
        "object-id",
        &encrypted_grant(51),
    )
    .await?;
    assert!(finalized.is_none());
    assert!(
        get_active_microsoft_mailbox_grant(&pool, OWNER_ID)
            .await?
            .is_none()
    );
    let state = sqlx::query!(
        "SELECT consumed_at, canceled_at, completed_at FROM microsoft_mailbox_oauth_flows WHERE flow_id = $1",
        flow_id,
    )
    .fetch_one(&pool)
    .await?;
    assert!(state.consumed_at.is_some());
    assert!(state.canceled_at.is_some());
    assert!(state.completed_at.is_none());
    Ok(())
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn callback_finalization_before_disconnect_is_then_locally_revoked(
    pool: Pool<Postgres>,
) -> anyhow::Result<()> {
    let flow_id = create_and_consume_flow(&pool, 52).await?;
    assert!(
        finalize_microsoft_mailbox_oauth_flow(
            &pool,
            flow_id,
            OWNER_ID,
            "owner@example.com",
            "tenant-id",
            "object-id",
            &encrypted_grant(53),
        )
        .await?
        .is_some()
    );
    assert!(disconnect_microsoft_mailbox_grant(&pool, OWNER_ID).await?);
    assert!(
        get_active_microsoft_mailbox_grant(&pool, OWNER_ID)
            .await?
            .is_none()
    );
    let state = sqlx::query!(
        "SELECT canceled_at, completed_at FROM microsoft_mailbox_oauth_flows WHERE flow_id = $1",
        flow_id,
    )
    .fetch_one(&pool)
    .await?;
    assert!(state.completed_at.is_some());
    assert!(state.canceled_at.is_none());
    Ok(())
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn fresh_consent_created_after_disconnect_can_finalize(
    pool: Pool<Postgres>,
) -> anyhow::Result<()> {
    assert!(!disconnect_microsoft_mailbox_grant(&pool, OWNER_ID).await?);
    let flow_id = create_and_consume_flow(&pool, 54).await?;
    assert!(
        finalize_microsoft_mailbox_oauth_flow(
            &pool,
            flow_id,
            OWNER_ID,
            "owner@example.com",
            "tenant-id",
            "object-id",
            &encrypted_grant(55),
        )
        .await?
        .is_some()
    );
    assert!(
        get_active_microsoft_mailbox_grant(&pool, OWNER_ID)
            .await?
            .is_some()
    );
    Ok(())
}
