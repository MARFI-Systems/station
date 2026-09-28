use models_email::email::service::microsoft::{MicrosoftMailbox, NewMicrosoftMailbox};
use sqlx::PgPool;
use sqlx::types::Uuid;

#[cfg(test)]
mod test;

/// Lock the owner-scoped active mailbox rows for the lifetime of the caller's transaction.
///
/// `FOR NO KEY UPDATE` serializes against disconnect/reactivation while remaining compatible
/// with foreign-key `KEY SHARE` locks taken by projection inserts on `email_links`.
pub(crate) async fn lock_active_microsoft_mailbox_for_owner(
    conn: &mut sqlx::PgConnection,
    owner_fusionauth_user_id: &str,
    link_id: Uuid,
) -> anyhow::Result<bool> {
    let active = sqlx::query_scalar::<_, bool>(
        r#"SELECT true
           FROM email_links l
           JOIN email_microsoft_mailboxes m ON m.link_id = l.id
           WHERE l.id = $1
             AND l.fusionauth_user_id = $2
             AND l.provider = 'MICROSOFT'::email_user_provider_enum
             AND l.is_sync_active = true
             AND m.disconnected_at IS NULL
           FOR NO KEY UPDATE OF l, m"#,
    )
    .bind(link_id)
    .bind(owner_fusionauth_user_id)
    .fetch_optional(conn)
    .await?;
    Ok(active.is_some())
}

/// Atomically create/reactivate the Microsoft email link and its mailbox identity.
///
/// Existing Gmail links are independent because the provider is part of the link
/// uniqueness key. Repeating the same provision request returns the same link.
#[tracing::instrument(err, skip(pool, link, mailbox))]
pub async fn provision_microsoft_mailbox(
    pool: &PgPool,
    link: models_email::service::link::Link,
    mailbox: NewMicrosoftMailbox,
) -> anyhow::Result<(models_email::service::link::Link, MicrosoftMailbox)> {
    if link.provider != models_email::service::link::UserProvider::Microsoft {
        anyhow::bail!("Microsoft mailbox provisioning requires a Microsoft link");
    }
    if link.id != mailbox.link_id {
        anyhow::bail!("Microsoft mailbox link identity mismatch");
    }

    let owner = link.fusionauth_user_id.clone();
    let mut tx = pool.begin().await?;
    let link = crate::links::insert::upsert_link(&mut tx, link).await?;
    let normalized_upn = mailbox.user_principal_name.to_lowercase();
    let persisted = sqlx::query_as!(
        MicrosoftMailbox,
        r#"
        INSERT INTO email_microsoft_mailboxes (
            link_id, tenant_id, mailbox_id, user_principal_name
        )
        SELECT l.id, $3, $4, $5
        FROM email_links l
        WHERE l.id = $1
          AND l.fusionauth_user_id = $2
          AND l.provider = 'MICROSOFT'::email_user_provider_enum
          AND lower(l.email_address) = $5
        ON CONFLICT (link_id) DO UPDATE SET
            tenant_id = EXCLUDED.tenant_id,
            mailbox_id = EXCLUDED.mailbox_id,
            user_principal_name = EXCLUDED.user_principal_name,
            disconnected_at = NULL,
            updated_at = now()
        RETURNING link_id, tenant_id, mailbox_id, user_principal_name,
                  disconnected_at, created_at, updated_at
        "#,
        link.id,
        owner,
        mailbox.tenant_id,
        mailbox.mailbox_id,
        normalized_upn,
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| anyhow::anyhow!("Microsoft mailbox owner or address mismatch"))?;
    tx.commit().await?;
    Ok((link, persisted))
}
/// Provision or reconnect the Microsoft mailbox attached to an owner-scoped link.
///
/// Returns `None` when the link is not a Microsoft link owned by the supplied
/// FusionAuth user, or when its email address does not match the normalized UPN.
#[tracing::instrument(err, skip(pool, mailbox), fields(link_id = %mailbox.link_id))]
pub async fn upsert_microsoft_mailbox_for_owner(
    pool: &PgPool,
    owner_fusionauth_user_id: &str,
    mailbox: NewMicrosoftMailbox,
) -> anyhow::Result<Option<MicrosoftMailbox>> {
    if owner_fusionauth_user_id.is_empty() {
        anyhow::bail!("owner FusionAuth user ID cannot be empty");
    }
    if mailbox.tenant_id.is_empty()
        || mailbox.mailbox_id.is_empty()
        || mailbox.user_principal_name.is_empty()
    {
        anyhow::bail!("Microsoft mailbox identity fields cannot be empty");
    }

    let normalized_upn = mailbox.user_principal_name.to_lowercase();
    let row = sqlx::query_as!(
        MicrosoftMailbox,
        r#"
        INSERT INTO email_microsoft_mailboxes (
            link_id, tenant_id, mailbox_id, user_principal_name
        )
        SELECT l.id, $3, $4, $5
        FROM email_links l
        WHERE l.id = $1
          AND l.fusionauth_user_id = $2
          AND l.provider = 'MICROSOFT'::email_user_provider_enum
          AND lower(l.email_address) = $5
        ON CONFLICT (link_id) DO UPDATE SET
            tenant_id = EXCLUDED.tenant_id,
            mailbox_id = EXCLUDED.mailbox_id,
            user_principal_name = EXCLUDED.user_principal_name,
            disconnected_at = NULL,
            updated_at = now()
        RETURNING link_id, tenant_id, mailbox_id, user_principal_name,
                  disconnected_at, created_at, updated_at
        "#,
        mailbox.link_id,
        owner_fusionauth_user_id,
        mailbox.tenant_id,
        mailbox.mailbox_id,
        normalized_upn,
    )
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

/// Fetch an active Microsoft mailbox only through its owning user boundary.
#[tracing::instrument(err, skip(pool))]
pub async fn fetch_active_microsoft_mailbox_for_owner(
    pool: &PgPool,
    owner_fusionauth_user_id: &str,
    link_id: Uuid,
) -> anyhow::Result<Option<MicrosoftMailbox>> {
    let row = sqlx::query_as!(
        MicrosoftMailbox,
        r#"
        SELECT m.link_id, m.tenant_id, m.mailbox_id, m.user_principal_name,
               m.disconnected_at, m.created_at, m.updated_at
        FROM email_microsoft_mailboxes m
        JOIN email_links l ON l.id = m.link_id
        WHERE m.link_id = $1
          AND l.fusionauth_user_id = $2
          AND l.provider = 'MICROSOFT'::email_user_provider_enum
          AND l.is_sync_active = true
          AND m.disconnected_at IS NULL
        "#,
        link_id,
        owner_fusionauth_user_id,
    )
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

/// Block local jobs for an owner-scoped Microsoft mailbox before credential revocation.
///
/// This does not delete or mutate provider data. The authentication service owns
/// refresh-token revocation/removal; callers must complete that step separately.
#[tracing::instrument(err, skip(pool))]
pub async fn disconnect_microsoft_mailbox_for_owner(
    pool: &PgPool,
    owner_fusionauth_user_id: &str,
    link_id: Uuid,
) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    let deactivated = sqlx::query_scalar!(
        r#"
        UPDATE email_links
        SET is_sync_active = false, updated_at = now()
        WHERE id = $1
          AND fusionauth_user_id = $2
          AND provider = 'MICROSOFT'::email_user_provider_enum
        RETURNING id
        "#,
        link_id,
        owner_fusionauth_user_id,
    )
    .fetch_optional(&mut *tx)
    .await?;

    if deactivated.is_none() {
        tx.rollback().await?;
        return Ok(false);
    }

    sqlx::query!(
        r#"
        UPDATE email_microsoft_mailboxes
        SET disconnected_at = COALESCE(disconnected_at, now()), updated_at = now()
        WHERE link_id = $1
        "#,
        link_id,
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(true)
}
