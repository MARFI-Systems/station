#![deny(missing_docs)]

//! Database utilities for encrypted Microsoft OAuth grants stored in MacroDB.

use chrono::{DateTime, Utc};
use sqlx::{Pool, Postgres, Transaction};
use uuid::Uuid;

#[cfg(test)]
mod test;

/// An encrypted Microsoft OAuth refresh-token envelope.
///
/// This type intentionally exposes only opaque encrypted bytes and envelope metadata.
/// Encryption, decryption, and plaintext token handling belong to the calling service.
#[derive(Clone, PartialEq, Eq)]
pub struct EncryptedMicrosoftOAuthGrant {
    refresh_token_ciphertext: Vec<u8>,
    encrypted_data_key: Vec<u8>,
    nonce: Vec<u8>,
    encryption_version: i32,
    kms_key_id: String,
}

impl EncryptedMicrosoftOAuthGrant {
    /// Creates an opaque encrypted grant envelope.
    pub fn new(
        refresh_token_ciphertext: Vec<u8>,
        encrypted_data_key: Vec<u8>,
        nonce: Vec<u8>,
        encryption_version: i32,
        kms_key_id: String,
    ) -> Self {
        Self {
            refresh_token_ciphertext,
            encrypted_data_key,
            nonce,
            encryption_version,
            kms_key_id,
        }
    }

    /// Returns the encrypted refresh-token bytes.
    pub fn refresh_token_ciphertext(&self) -> &[u8] {
        &self.refresh_token_ciphertext
    }

    /// Returns the KMS-encrypted data-key bytes.
    pub fn encrypted_data_key(&self) -> &[u8] {
        &self.encrypted_data_key
    }

    /// Returns the AES-GCM nonce.
    pub fn nonce(&self) -> &[u8] {
        &self.nonce
    }

    /// Returns the envelope encryption format version.
    pub fn encryption_version(&self) -> i32 {
        self.encryption_version
    }

    /// Returns the KMS key identifier used to create the data key.
    pub fn kms_key_id(&self) -> &str {
        &self.kms_key_id
    }
}

/// An encrypted Microsoft OAuth grant as stored in MacroDB.
pub struct StoredMicrosoftOAuthGrant {
    fusionauth_user_id: String,
    email_address: String,
    encrypted_grant: EncryptedMicrosoftOAuthGrant,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    last_refreshed_at: DateTime<Utc>,
    microsoft_tenant_id: Option<String>,
    microsoft_object_id: Option<String>,
}

impl StoredMicrosoftOAuthGrant {
    /// Returns the FusionAuth user that owns the grant.
    pub fn fusionauth_user_id(&self) -> &str {
        &self.fusionauth_user_id
    }

    /// Returns the normalized mailbox email address.
    pub fn email_address(&self) -> &str {
        &self.email_address
    }

    /// Returns the opaque encrypted grant envelope.
    pub fn encrypted_grant(&self) -> &EncryptedMicrosoftOAuthGrant {
        &self.encrypted_grant
    }

    /// Returns when the grant was first stored.
    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    /// Returns when the stored envelope was last replaced.
    pub fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    /// Returns when the refresh-token grant was last received from Microsoft.
    pub fn last_refreshed_at(&self) -> DateTime<Utc> {
        self.last_refreshed_at
    }

    /// Returns the verified Entra tenant identifier for a mailbox grant.
    pub fn microsoft_tenant_id(&self) -> Option<&str> {
        self.microsoft_tenant_id.as_deref()
    }

    /// Returns the verified Entra object identifier for a mailbox grant.
    pub fn microsoft_object_id(&self) -> Option<&str> {
        self.microsoft_object_id.as_deref()
    }
}

struct MicrosoftOAuthGrantRow {
    fusionauth_user_id: String,
    email_address: String,
    refresh_token_ciphertext: Vec<u8>,
    encrypted_data_key: Vec<u8>,
    nonce: Vec<u8>,
    encryption_version: i32,
    kms_key_id: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    last_refreshed_at: DateTime<Utc>,
    microsoft_tenant_id: Option<String>,
    microsoft_object_id: Option<String>,
}

impl From<MicrosoftOAuthGrantRow> for StoredMicrosoftOAuthGrant {
    fn from(row: MicrosoftOAuthGrantRow) -> Self {
        Self {
            fusionauth_user_id: row.fusionauth_user_id,
            email_address: row.email_address,
            encrypted_grant: EncryptedMicrosoftOAuthGrant::new(
                row.refresh_token_ciphertext,
                row.encrypted_data_key,
                row.nonce,
                row.encryption_version,
                row.kms_key_id,
            ),
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_refreshed_at: row.last_refreshed_at,
            microsoft_tenant_id: row.microsoft_tenant_id,
            microsoft_object_id: row.microsoft_object_id,
        }
    }
}

/// Inserts an encrypted Microsoft OAuth grant or atomically replaces its complete envelope.
pub async fn upsert_microsoft_oauth_grant(
    db: &Pool<Postgres>,
    fusionauth_user_id: &str,
    email_address: &str,
    encrypted_grant: &EncryptedMicrosoftOAuthGrant,
) -> anyhow::Result<StoredMicrosoftOAuthGrant> {
    let email_address = normalize_email_address(email_address);
    let row = sqlx::query_as!(
        MicrosoftOAuthGrantRow,
        r#"
            INSERT INTO microsoft_oauth_grants (
                fusionauth_user_id,
                email_address,
                refresh_token_ciphertext,
                encrypted_data_key,
                nonce,
                encryption_version,
                kms_key_id,
                grant_purpose,
                grant_schema_version
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, 'legacy_identity', 0)
            ON CONFLICT (fusionauth_user_id, email_address) DO UPDATE SET
                refresh_token_ciphertext = EXCLUDED.refresh_token_ciphertext,
                encrypted_data_key = EXCLUDED.encrypted_data_key,
                nonce = EXCLUDED.nonce,
                encryption_version = EXCLUDED.encryption_version,
                kms_key_id = EXCLUDED.kms_key_id,
                grant_purpose = 'legacy_identity',
                grant_schema_version = 0,
                microsoft_tenant_id = NULL,
                microsoft_object_id = NULL,
                disconnected_at = NULL,
                updated_at = now(),
                last_refreshed_at = now()
            RETURNING
                fusionauth_user_id,
                email_address,
                refresh_token_ciphertext,
                encrypted_data_key,
                nonce,
                encryption_version,
                kms_key_id,
                created_at,
                updated_at,
                last_refreshed_at,
                microsoft_tenant_id,
                microsoft_object_id
        "#,
        fusionauth_user_id,
        email_address,
        encrypted_grant.refresh_token_ciphertext,
        encrypted_grant.encrypted_data_key,
        encrypted_grant.nonce,
        encrypted_grant.encryption_version,
        encrypted_grant.kms_key_id,
    )
    .fetch_one(db)
    .await?;

    Ok(row.into())
}

/// Fetches an encrypted Microsoft OAuth grant by owner and normalized mailbox.
pub async fn get_microsoft_oauth_grant(
    db: &Pool<Postgres>,
    fusionauth_user_id: &str,
    email_address: &str,
) -> anyhow::Result<Option<StoredMicrosoftOAuthGrant>> {
    let email_address = normalize_email_address(email_address);
    let row = sqlx::query_as!(
        MicrosoftOAuthGrantRow,
        r#"
            SELECT
                fusionauth_user_id,
                email_address,
                refresh_token_ciphertext,
                encrypted_data_key,
                nonce,
                encryption_version,
                kms_key_id,
                created_at,
                updated_at,
                last_refreshed_at,
                microsoft_tenant_id,
                microsoft_object_id
            FROM microsoft_oauth_grants
            WHERE fusionauth_user_id = $1
              AND email_address = $2
              AND grant_purpose = 'mailbox_read'
              AND grant_schema_version = 1
              AND disconnected_at IS NULL
        "#,
        fusionauth_user_id,
        email_address,
    )
    .fetch_optional(db)
    .await?;

    Ok(row.map(Into::into))
}

fn normalize_email_address(email_address: &str) -> String {
    email_address.to_lowercase()
}

/// Atomically replaces a grant only when the caller still holds the fetched envelope.
pub async fn replace_microsoft_oauth_grant_if_current(
    db: &Pool<Postgres>,
    fusionauth_user_id: &str,
    email_address: &str,
    expected: &EncryptedMicrosoftOAuthGrant,
    replacement: &EncryptedMicrosoftOAuthGrant,
) -> anyhow::Result<bool> {
    let email_address = normalize_email_address(email_address);
    let result = sqlx::query!(
        r#"
            UPDATE microsoft_oauth_grants SET
                refresh_token_ciphertext = $4,
                encrypted_data_key = $5,
                nonce = $6,
                encryption_version = $7,
                kms_key_id = $8,
                updated_at = now(),
                last_refreshed_at = now()
            WHERE fusionauth_user_id = $1
              AND email_address = $2
              AND refresh_token_ciphertext = $3
              AND grant_purpose = 'mailbox_read'
              AND grant_schema_version = 1
              AND disconnected_at IS NULL
        "#,
        fusionauth_user_id,
        email_address,
        expected.refresh_token_ciphertext,
        replacement.refresh_token_ciphertext,
        replacement.encrypted_data_key,
        replacement.nonce,
        replacement.encryption_version,
        replacement.kms_key_id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Deletes a revoked grant only when it has not already been rotated by another request.
pub async fn delete_microsoft_oauth_grant_if_current(
    db: &Pool<Postgres>,
    fusionauth_user_id: &str,
    email_address: &str,
    expected: &EncryptedMicrosoftOAuthGrant,
) -> anyhow::Result<bool> {
    let email_address = normalize_email_address(email_address);
    let result = sqlx::query!(
        r#"
            DELETE FROM microsoft_oauth_grants
            WHERE fusionauth_user_id = $1
              AND email_address = $2
              AND refresh_token_ciphertext = $3
              AND grant_purpose = 'mailbox_read'
              AND grant_schema_version = 1
              AND disconnected_at IS NULL
        "#,
        fusionauth_user_id,
        email_address,
        expected.refresh_token_ciphertext,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// A consumed, owner-bound Microsoft mailbox OAuth flow.
pub struct ConsumedMicrosoftMailboxOAuthFlow {
    encrypted_code_verifier: EncryptedMicrosoftOAuthGrant,
}

impl ConsumedMicrosoftMailboxOAuthFlow {
    /// Returns the encrypted PKCE verifier.
    pub fn encrypted_code_verifier(&self) -> &EncryptedMicrosoftOAuthGrant {
        &self.encrypted_code_verifier
    }
}

struct MicrosoftMailboxOAuthFlowRow {
    verifier_ciphertext: Vec<u8>,
    encrypted_data_key: Vec<u8>,
    nonce: Vec<u8>,
    encryption_version: i32,
    kms_key_id: String,
}

/// Creates a bounded server-side OAuth flow. Expiry must be in the next ten minutes.
pub async fn create_microsoft_mailbox_oauth_flow(
    db: &Pool<Postgres>,
    flow_id: Uuid,
    fusionauth_user_id: &str,
    state_hash: &[u8],
    encrypted_code_verifier: &EncryptedMicrosoftOAuthGrant,
    expires_at: DateTime<Utc>,
) -> anyhow::Result<()> {
    let now = Utc::now();
    if expires_at <= now || expires_at > now + chrono::Duration::minutes(10) {
        anyhow::bail!("Microsoft mailbox OAuth flow expiry is outside the allowed window");
    }
    let mut transaction = db.begin().await?;
    lock_microsoft_mailbox_owner(&mut transaction, fusionauth_user_id).await?;
    sqlx::query!(
        r#"
            INSERT INTO microsoft_mailbox_oauth_flows (
                flow_id, fusionauth_user_id, state_hash, verifier_ciphertext,
                encrypted_data_key, nonce, encryption_version, kms_key_id, expires_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
        "#,
        flow_id,
        fusionauth_user_id,
        state_hash,
        encrypted_code_verifier.refresh_token_ciphertext,
        encrypted_code_verifier.encrypted_data_key,
        encrypted_code_verifier.nonce,
        encrypted_code_verifier.encryption_version,
        encrypted_code_verifier.kms_key_id,
        expires_at,
    )
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

/// Atomically consumes an unexpired flow exactly once for its authenticated owner and state hash.
pub async fn consume_microsoft_mailbox_oauth_flow(
    db: &Pool<Postgres>,
    flow_id: Uuid,
    fusionauth_user_id: &str,
    state_hash: &[u8],
) -> anyhow::Result<Option<ConsumedMicrosoftMailboxOAuthFlow>> {
    let row = sqlx::query_as!(
        MicrosoftMailboxOAuthFlowRow,
        r#"
            UPDATE microsoft_mailbox_oauth_flows
            SET consumed_at = now()
            WHERE flow_id = $1
              AND fusionauth_user_id = $2
              AND state_hash = $3
              AND consumed_at IS NULL
              AND canceled_at IS NULL
              AND completed_at IS NULL
              AND expires_at >= now()
              AND expires_at <= created_at + interval '10 minutes'
            RETURNING verifier_ciphertext, encrypted_data_key, nonce,
                      encryption_version, kms_key_id
        "#,
        flow_id,
        fusionauth_user_id,
        state_hash,
    )
    .fetch_optional(db)
    .await?;
    Ok(row.map(|row| ConsumedMicrosoftMailboxOAuthFlow {
        encrypted_code_verifier: EncryptedMicrosoftOAuthGrant::new(
            row.verifier_ciphertext,
            row.encrypted_data_key,
            row.nonce,
            row.encryption_version,
            row.kms_key_id,
        ),
    }))
}

/// Stores a verified delegated mailbox grant and disconnects any previous mailbox for the owner.
async fn upsert_microsoft_mailbox_grant(
    db: &Pool<Postgres>,
    fusionauth_user_id: &str,
    email_address: &str,
    microsoft_tenant_id: &str,
    microsoft_object_id: &str,
    encrypted_grant: &EncryptedMicrosoftOAuthGrant,
) -> anyhow::Result<StoredMicrosoftOAuthGrant> {
    let email_address = normalize_email_address(email_address);
    let mut transaction = db.begin().await?;
    lock_microsoft_mailbox_owner(&mut transaction, fusionauth_user_id).await?;
    sqlx::query!(
        r#"UPDATE microsoft_oauth_grants
           SET disconnected_at = now(), updated_at = now()
           WHERE fusionauth_user_id = $1
             AND grant_purpose = 'mailbox_read'
             AND disconnected_at IS NULL"#,
        fusionauth_user_id,
    )
    .execute(&mut *transaction)
    .await?;
    let row = sqlx::query_as!(
        MicrosoftOAuthGrantRow,
        r#"
            INSERT INTO microsoft_oauth_grants (
                fusionauth_user_id, email_address, refresh_token_ciphertext,
                encrypted_data_key, nonce, encryption_version, kms_key_id,
                grant_purpose, grant_schema_version, microsoft_tenant_id,
                microsoft_object_id, disconnected_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,'mailbox_read',1,$8,$9,NULL)
            ON CONFLICT (fusionauth_user_id, email_address) DO UPDATE SET
                refresh_token_ciphertext = EXCLUDED.refresh_token_ciphertext,
                encrypted_data_key = EXCLUDED.encrypted_data_key,
                nonce = EXCLUDED.nonce,
                encryption_version = EXCLUDED.encryption_version,
                kms_key_id = EXCLUDED.kms_key_id,
                grant_purpose = 'mailbox_read', grant_schema_version = 1,
                microsoft_tenant_id = EXCLUDED.microsoft_tenant_id,
                microsoft_object_id = EXCLUDED.microsoft_object_id,
                disconnected_at = NULL, updated_at = now(), last_refreshed_at = now()
            RETURNING fusionauth_user_id, email_address, refresh_token_ciphertext,
                encrypted_data_key, nonce, encryption_version, kms_key_id,
                created_at, updated_at, last_refreshed_at,
                microsoft_tenant_id, microsoft_object_id
        "#,
        fusionauth_user_id,
        email_address,
        encrypted_grant.refresh_token_ciphertext,
        encrypted_grant.encrypted_data_key,
        encrypted_grant.nonce,
        encrypted_grant.encryption_version,
        encrypted_grant.kms_key_id,
        microsoft_tenant_id,
        microsoft_object_id,
    )
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(row.into())
}

/// Fetches the single active, purpose-versioned mailbox grant for an owner.
pub async fn get_active_microsoft_mailbox_grant(
    db: &Pool<Postgres>,
    fusionauth_user_id: &str,
) -> anyhow::Result<Option<StoredMicrosoftOAuthGrant>> {
    let row = sqlx::query_as!(
        MicrosoftOAuthGrantRow,
        r#"SELECT fusionauth_user_id, email_address, refresh_token_ciphertext,
                  encrypted_data_key, nonce, encryption_version, kms_key_id,
                  created_at, updated_at, last_refreshed_at,
                  microsoft_tenant_id, microsoft_object_id
           FROM microsoft_oauth_grants
           WHERE fusionauth_user_id = $1
             AND grant_purpose = 'mailbox_read'
             AND grant_schema_version = 1
             AND disconnected_at IS NULL"#,
        fusionauth_user_id,
    )
    .fetch_optional(db)
    .await?;
    Ok(row.map(Into::into))
}

/// Confirms a fetched grant is still active before returning a refreshed access token.
pub async fn microsoft_mailbox_grant_is_current(
    db: &Pool<Postgres>,
    fusionauth_user_id: &str,
    expected: &EncryptedMicrosoftOAuthGrant,
) -> anyhow::Result<bool> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS(
            SELECT 1 FROM microsoft_oauth_grants
            WHERE fusionauth_user_id = $1
              AND refresh_token_ciphertext = $2
              AND grant_purpose = 'mailbox_read'
              AND grant_schema_version = 1
              AND disconnected_at IS NULL
        ) AS "exists!""#,
        fusionauth_user_id,
        expected.refresh_token_ciphertext,
    )
    .fetch_one(db)
    .await?;
    Ok(exists)
}

/// Deletes only the active local mailbox grant for an owner. No provider revocation is attempted.
pub async fn disconnect_microsoft_mailbox_grant(
    db: &Pool<Postgres>,
    fusionauth_user_id: &str,
) -> anyhow::Result<bool> {
    let mut transaction = db.begin().await?;
    lock_microsoft_mailbox_owner(&mut transaction, fusionauth_user_id).await?;
    sqlx::query!(
        r#"UPDATE microsoft_mailbox_oauth_flows
           SET canceled_at = now()
           WHERE fusionauth_user_id = $1
             AND completed_at IS NULL
             AND canceled_at IS NULL"#,
        fusionauth_user_id,
    )
    .execute(&mut *transaction)
    .await?;
    let result = sqlx::query!(
        r#"DELETE FROM microsoft_oauth_grants
           WHERE fusionauth_user_id = $1
             AND grant_purpose = 'mailbox_read'
             AND grant_schema_version = 1
             AND disconnected_at IS NULL"#,
        fusionauth_user_id,
    )
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(result.rows_affected() == 1)
}

/// Finalizes a consumed OAuth flow and stores its grant under the same owner lock as disconnect.
pub async fn finalize_microsoft_mailbox_oauth_flow(
    db: &Pool<Postgres>,
    flow_id: Uuid,
    fusionauth_user_id: &str,
    email_address: &str,
    microsoft_tenant_id: &str,
    microsoft_object_id: &str,
    encrypted_grant: &EncryptedMicrosoftOAuthGrant,
) -> anyhow::Result<Option<StoredMicrosoftOAuthGrant>> {
    let email_address = normalize_email_address(email_address);
    let mut transaction = db.begin().await?;
    lock_microsoft_mailbox_owner(&mut transaction, fusionauth_user_id).await?;
    let finalizable = sqlx::query_scalar!(
        r#"SELECT flow_id
           FROM microsoft_mailbox_oauth_flows
           WHERE flow_id = $1
             AND fusionauth_user_id = $2
             AND consumed_at IS NOT NULL
             AND canceled_at IS NULL
             AND completed_at IS NULL
             AND expires_at >= now()
             AND expires_at <= created_at + interval '10 minutes'
           FOR UPDATE"#,
        flow_id,
        fusionauth_user_id,
    )
    .fetch_optional(&mut *transaction)
    .await?;
    if finalizable.is_none() {
        transaction.rollback().await?;
        return Ok(None);
    }

    sqlx::query!(
        r#"UPDATE microsoft_oauth_grants
           SET disconnected_at = now(), updated_at = now()
           WHERE fusionauth_user_id = $1
             AND grant_purpose = 'mailbox_read'
             AND disconnected_at IS NULL"#,
        fusionauth_user_id,
    )
    .execute(&mut *transaction)
    .await?;
    let row = sqlx::query_as!(
        MicrosoftOAuthGrantRow,
        r#"
            INSERT INTO microsoft_oauth_grants (
                fusionauth_user_id, email_address, refresh_token_ciphertext,
                encrypted_data_key, nonce, encryption_version, kms_key_id,
                grant_purpose, grant_schema_version, microsoft_tenant_id,
                microsoft_object_id, disconnected_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,'mailbox_read',1,$8,$9,NULL)
            ON CONFLICT (fusionauth_user_id, email_address) DO UPDATE SET
                refresh_token_ciphertext = EXCLUDED.refresh_token_ciphertext,
                encrypted_data_key = EXCLUDED.encrypted_data_key,
                nonce = EXCLUDED.nonce,
                encryption_version = EXCLUDED.encryption_version,
                kms_key_id = EXCLUDED.kms_key_id,
                grant_purpose = 'mailbox_read', grant_schema_version = 1,
                microsoft_tenant_id = EXCLUDED.microsoft_tenant_id,
                microsoft_object_id = EXCLUDED.microsoft_object_id,
                disconnected_at = NULL, updated_at = now(), last_refreshed_at = now()
            RETURNING fusionauth_user_id, email_address, refresh_token_ciphertext,
                encrypted_data_key, nonce, encryption_version, kms_key_id,
                created_at, updated_at, last_refreshed_at,
                microsoft_tenant_id, microsoft_object_id
        "#,
        fusionauth_user_id,
        email_address,
        encrypted_grant.refresh_token_ciphertext,
        encrypted_grant.encrypted_data_key,
        encrypted_grant.nonce,
        encrypted_grant.encryption_version,
        encrypted_grant.kms_key_id,
        microsoft_tenant_id,
        microsoft_object_id,
    )
    .fetch_one(&mut *transaction)
    .await?;
    sqlx::query!(
        r#"UPDATE microsoft_mailbox_oauth_flows
           SET completed_at = now()
           WHERE flow_id = $1 AND fusionauth_user_id = $2"#,
        flow_id,
        fusionauth_user_id,
    )
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(Some(row.into()))
}

async fn lock_microsoft_mailbox_owner(
    transaction: &mut Transaction<'_, Postgres>,
    fusionauth_user_id: &str,
) -> anyhow::Result<()> {
    sqlx::query!(
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        fusionauth_user_id,
    )
    .execute(&mut **transaction)
    .await?;
    Ok(())
}
