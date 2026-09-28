use super::{EmailServiceImpl, MAX_SENDER_ADDRESS_LEN, changed_project_ids};
use crate::domain::models::EmailErr;

#[test]
fn changed_project_ids_include_old_and_new_projects() {
    assert_eq!(
        changed_project_ids(Some("old-project"), Some("new-project")),
        vec!["old-project", "new-project"]
    );
}

#[test]
fn changed_project_ids_include_only_the_project_for_root_transitions() {
    assert_eq!(
        changed_project_ids(None, Some("new-project")),
        vec!["new-project"]
    );
    assert_eq!(
        changed_project_ids(Some("old-project"), None),
        vec!["old-project"]
    );
}

#[test]
fn changed_project_ids_are_empty_for_unchanged_assignments() {
    assert!(changed_project_ids(None, None).is_empty());
    assert!(changed_project_ids(Some("project"), Some("project")).is_empty());
}

#[test]
fn changed_project_ids_ignore_empty_and_duplicate_ids() {
    assert!(changed_project_ids(Some(""), None).is_empty());
    assert!(changed_project_ids(Some("project"), Some("project")).is_empty());
}

fn validate_sender_address(addr: &str) -> Result<String, EmailErr> {
    EmailServiceImpl::<(), (), (), (), (), ()>::validate_sender_address(addr)
}

#[test]
fn validate_sender_address_rejects_empty() {
    match validate_sender_address("") {
        Err(EmailErr::InvalidEmailFilter(msg)) => {
            assert_eq!(msg, "Email address cannot be empty");
        }
        other => panic!("expected InvalidEmailFilter, got {other:?}"),
    }
    match validate_sender_address("   ") {
        Err(EmailErr::InvalidEmailFilter(msg)) => {
            assert_eq!(msg, "Email address cannot be empty");
        }
        other => panic!("expected InvalidEmailFilter, got {other:?}"),
    }
}

#[test]
fn validate_sender_address_rejects_missing_at() {
    match validate_sender_address("not-an-email") {
        Err(EmailErr::InvalidEmailFilter(msg)) => {
            assert_eq!(msg, "Invalid email address format");
        }
        other => panic!("expected InvalidEmailFilter, got {other:?}"),
    }
}

#[test]
fn validate_sender_address_rejects_too_long() {
    let addr = format!("{}@example.com", "a".repeat(MAX_SENDER_ADDRESS_LEN));
    match validate_sender_address(&addr) {
        Err(EmailErr::InvalidEmailFilter(msg)) => {
            assert_eq!(msg, "Email address is too long");
        }
        other => panic!("expected InvalidEmailFilter, got {other:?}"),
    }
}

#[test]
fn validate_sender_address_trims_and_lowercases() {
    assert_eq!(
        validate_sender_address("  Teo@Macro.COM  ").unwrap(),
        "teo@macro.com"
    );
}

fn provider_link(provider: crate::domain::models::UserProvider) -> crate::domain::models::Link {
    crate::domain::models::Link {
        id: uuid::Uuid::nil(),
        macro_id: macro_user_id::user_id::MacroUserIdStr::try_from(
            "macro|owner@example.com".to_owned(),
        )
        .unwrap(),
        fusionauth_user_id: "owner".to_owned(),
        email_address: macro_user_id::email::EmailStr::try_from("owner@example.com".to_owned())
            .unwrap(),
        provider,
        is_sync_active: true,
        is_primary: true,
        created_at: Default::default(),
        updated_at: Default::default(),
    }
}

#[test]
fn microsoft_provider_denies_mutations() {
    assert!(matches!(
        super::ensure_provider_writable(&provider_link(
            crate::domain::models::UserProvider::Microsoft,
        )),
        Err(EmailErr::ProviderReadOnly)
    ));
}

#[test]
fn gmail_provider_remains_writable() {
    super::ensure_provider_writable(&provider_link(crate::domain::models::UserProvider::Gmail))
        .expect("Gmail mutation behavior must not regress");
}
