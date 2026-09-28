use models_email::service::link::{Link, UserProvider};
use thiserror::Error;

#[derive(Debug, Error)]
#[error("Microsoft mailboxes are read-only")]
pub struct ProviderReadOnly;

/// Must run after resolving the link that actually owns the mutated entity, not merely a
/// caller-selected/default link. This prevents a writable Gmail selection from authorizing a
/// mutation against a Microsoft-owned thread, message, draft, or label ID.
pub fn ensure_writable(link: &Link) -> Result<(), ProviderReadOnly> {
    if link.provider == UserProvider::Microsoft {
        Err(ProviderReadOnly)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use macro_user_id::{email::EmailStr, user_id::MacroUserIdStr};

    fn link(provider: UserProvider) -> Link {
        Link {
            id: uuid::Uuid::nil(),
            macro_id: MacroUserIdStr::try_from("macro|owner@example.com".to_string()).unwrap(),
            fusionauth_user_id: "owner".into(),
            email_address: EmailStr::try_from("owner@example.com".to_string()).unwrap(),
            provider,
            is_sync_active: true,
            is_primary: true,
            needs_reauth: false,
            last_sync_error_at: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn gmail_remains_writable_and_microsoft_is_denied() {
        assert!(ensure_writable(&link(UserProvider::Gmail)).is_ok());
        assert_eq!(
            ensure_writable(&link(UserProvider::Microsoft))
                .unwrap_err()
                .to_string(),
            "Microsoft mailboxes are read-only"
        );
    }
}
