use std::{
    collections::{HashMap, VecDeque},
    sync::Mutex,
};

use fusionauth::microsoft::oauth::{MicrosoftExchangeTokenResponse, MicrosoftUserInfo};
use reqwest::StatusCode;
use uuid::Uuid;

use super::*;
use crate::api::oauth2::OAuthState;

const IDENTITY_PROVIDER_ID: &str = "microsoft-idp-id";
const PENDING_OWNER: &str = "pending-owner";
const REFRESH_TOKEN: &str = "private-microsoft-refresh-token";

fn state(identity_provider_id: &str, link_id: Option<Uuid>) -> OAuthState {
    OAuthState {
        identity_provider_id: identity_provider_id.to_string(),
        link_id,
        original_url: None,
        is_mobile: None,
    }
}

#[test]
fn extracts_subject_and_normalizes_email() {
    let identity = extract_identity(MicrosoftUserInfo {
        sub: "microsoft-user-id".into(),
        tenant_id: "microsoft-tenant-id".into(),
        object_id: "microsoft-object-id".into(),
        email: "Linked.User+Macro@Example.COM".into(),
    })
    .unwrap();

    assert_eq!(identity.subject, "microsoft-user-id");
    assert_eq!(identity.email, "linked.user@example.com");
}

#[test]
fn rejects_identity_without_subject_or_usable_email() {
    for user_info in [
        MicrosoftUserInfo {
            sub: "".into(),
            tenant_id: "microsoft-tenant-id".into(),
            object_id: "microsoft-object-id".into(),
            email: "linked@example.com".into(),
        },
        MicrosoftUserInfo {
            sub: "microsoft-user-id".into(),
            tenant_id: "microsoft-tenant-id".into(),
            object_id: "microsoft-object-id".into(),
            email: "not-an-email".into(),
        },
    ] {
        assert!(extract_identity(user_info).is_err());
    }
}

#[test]
fn microsoft_callback_requires_link_id() {
    let error = require_link_id(&state(IDENTITY_PROVIDER_ID, None)).unwrap_err();

    assert_eq!(error.0, StatusCode::BAD_REQUEST);
    assert!(error.1.contains("link_id"));
}

#[test]
fn microsoft_callback_rejects_identity_provider_mismatch() {
    let callback_state = state("unexpected-idp-id", Some(Uuid::now_v7()));
    let error = verify_identity_provider(&callback_state, IDENTITY_PROVIDER_ID).unwrap_err();

    assert_eq!(error.0, StatusCode::BAD_REQUEST);
}

#[test]
fn microsoft_callback_accepts_resolved_identity_provider() {
    let callback_state = state(IDENTITY_PROVIDER_ID, Some(Uuid::now_v7()));

    verify_identity_provider(&callback_state, IDENTITY_PROVIDER_ID).unwrap();
}

#[tokio::test]
async fn identity_link_never_persists_a_mailbox_grant() {
    let dependencies = FakeDependencies::new(
        [REFRESH_TOKEN],
        [LinkScenario::Fresh],
        "Linked.User@Example.COM",
    );
    let link_id = Uuid::now_v7();

    let response = handler_with_dependencies(
        &dependencies,
        "authorization-code",
        &state(IDENTITY_PROVIDER_ID, Some(link_id)),
    )
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let recorded = dependencies.recorded();
    assert_eq!(recorded.events, ["link", "mark"]);
    assert!(recorded.grants.is_empty());
    assert!(recorded.encryption_identities.is_empty());
    assert_eq!(recorded.fusion_token_lengths, [REFRESH_TOKEN.len()]);
    assert!(recorded.compensations.is_empty());
    assert!(recorded.cleaned_links.is_empty());
}

#[tokio::test]
async fn reconnect_updates_identity_link_without_touching_mailbox_grants() {
    let dependencies = FakeDependencies::new(
        ["first-refresh-token", "replacement-refresh-token"],
        [
            LinkScenario::Fresh,
            LinkScenario::Reconnect("first-refresh-token".into()),
        ],
        "linked@example.com",
    );

    for _ in 0..2 {
        handler_with_dependencies(
            &dependencies,
            "authorization-code",
            &state(IDENTITY_PROVIDER_ID, Some(Uuid::now_v7())),
        )
        .await
        .unwrap();
    }

    let recorded = dependencies.recorded();
    assert!(recorded.grants.is_empty());
    assert_eq!(recorded.events, ["link", "mark", "link", "mark"]);
}

#[tokio::test]
async fn identity_link_resolves_owner_but_does_not_create_mailbox_grant() {
    let dependencies = FakeDependencies::new(
        [REFRESH_TOKEN],
        [LinkScenario::Fresh],
        "Linked.User+Alias@Example.COM",
    )
    .with_mailbox_owner("mailbox-owner");

    handler_with_dependencies(
        &dependencies,
        "authorization-code",
        &state(IDENTITY_PROVIDER_ID, Some(Uuid::now_v7())),
    )
    .await
    .unwrap();

    let recorded = dependencies.recorded();
    assert!(recorded.encryption_identities.is_empty());
    assert!(recorded.grants.is_empty());
}

#[tokio::test]
async fn callback_failure_before_linking_still_cleans_pending_link() {
    let dependencies =
        FakeDependencies::new([REFRESH_TOKEN], [LinkScenario::Fresh], "not-an-email");
    let link_id = Uuid::now_v7();

    let response = callback_error(
        handler_with_dependencies(
            &dependencies,
            "authorization-code",
            &state(IDENTITY_PROVIDER_ID, Some(link_id)),
        )
        .await,
    );

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(dependencies.recorded().cleaned_links, [link_id]);
}

fn callback_error(result: Result<Response, Response>) -> Response {
    match result {
        Ok(_) => panic!("callback unexpectedly succeeded"),
        Err(response) => response,
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Failure {
    None,
    Encryption,
    Persistence,
}

enum LinkScenario {
    Fresh,
    Reconnect(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Compensation {
    Fresh,
    Replaced,
    Unchanged,
}

struct FakeDependencies {
    state: Mutex<FakeState>,
    user_email: String,
    mailbox_owner: Option<String>,
    failure: Failure,
}

struct FakeState {
    refresh_tokens: VecDeque<String>,
    link_scenarios: VecDeque<LinkScenario>,
    events: Vec<&'static str>,
    grants: HashMap<(String, String), Vec<u8>>,
    encryption_count: u8,
    encryption_identities: Vec<(String, String)>,
    fusion_token_lengths: Vec<usize>,
    marked_emails: Vec<String>,
    compensations: Vec<Compensation>,
    cleaned_links: Vec<Uuid>,
}

struct RecordedState {
    events: Vec<&'static str>,
    grants: HashMap<(String, String), Vec<u8>>,
    encryption_identities: Vec<(String, String)>,
    fusion_token_lengths: Vec<usize>,
    marked_emails: Vec<String>,
    compensations: Vec<Compensation>,
    cleaned_links: Vec<Uuid>,
}

impl FakeDependencies {
    fn new(
        refresh_tokens: impl IntoIterator<Item = &'static str>,
        link_scenarios: impl IntoIterator<Item = LinkScenario>,
        user_email: &str,
    ) -> Self {
        Self {
            state: Mutex::new(FakeState {
                refresh_tokens: refresh_tokens.into_iter().map(str::to_owned).collect(),
                link_scenarios: link_scenarios.into_iter().collect(),
                events: Vec::new(),
                grants: HashMap::new(),
                encryption_count: 0,
                encryption_identities: Vec::new(),
                fusion_token_lengths: Vec::new(),
                marked_emails: Vec::new(),
                compensations: Vec::new(),
                cleaned_links: Vec::new(),
            }),
            user_email: user_email.to_owned(),
            mailbox_owner: None,
            failure: Failure::None,
        }
    }

    fn with_mailbox_owner(mut self, mailbox_owner: &str) -> Self {
        self.mailbox_owner = Some(mailbox_owner.to_owned());
        self
    }

    fn with_failure(mut self, failure: Failure) -> Self {
        self.failure = failure;
        self
    }

    fn recorded(&self) -> RecordedState {
        let state = self.state.lock().unwrap();
        RecordedState {
            events: state.events.clone(),
            grants: state.grants.clone(),
            encryption_identities: state.encryption_identities.clone(),
            fusion_token_lengths: state.fusion_token_lengths.clone(),
            marked_emails: state.marked_emails.clone(),
            compensations: state.compensations.clone(),
            cleaned_links: state.cleaned_links.clone(),
        }
    }
}

#[async_trait::async_trait]
impl MicrosoftCallbackDependencies for FakeDependencies {
    async fn identity_provider_id(&self) -> MicrosoftCallbackResult<String> {
        Ok(IDENTITY_PROVIDER_ID.into())
    }

    async fn pending_link_owner(&self, _link_id: &Uuid) -> MicrosoftCallbackResult<String> {
        Ok(PENDING_OWNER.into())
    }

    async fn exchange_tokens(
        &self,
        _code: &str,
    ) -> MicrosoftCallbackResult<MicrosoftExchangeTokenResponse> {
        let refresh_token = self
            .state
            .lock()
            .unwrap()
            .refresh_tokens
            .pop_front()
            .expect("a refresh token fixture");
        Ok(MicrosoftExchangeTokenResponse {
            refresh_token,
            id_token: "microsoft-id-token".into(),
        })
    }

    async fn parse_identity(&self, _id_token: &str) -> MicrosoftCallbackResult<MicrosoftUserInfo> {
        Ok(MicrosoftUserInfo {
            sub: "microsoft-user-id".into(),
            tenant_id: "microsoft-tenant-id".into(),
            object_id: "microsoft-object-id".into(),
            email: self.user_email.clone(),
        })
    }

    async fn mailbox_owner(
        &self,
        _email: &str,
        pending_link_owner: &str,
    ) -> MicrosoftCallbackResult<String> {
        Ok(self
            .mailbox_owner
            .clone()
            .unwrap_or_else(|| pending_link_owner.to_owned()))
    }

    async fn link_identity(
        &self,
        _identity_provider_id: &str,
        _link_owner_id: &str,
        _identity: &MicrosoftLinkIdentity,
        refresh_token: &str,
    ) -> MicrosoftCallbackResult<FusionLinkChange> {
        let mut state = self.state.lock().unwrap();
        state.events.push("link");
        state.fusion_token_lengths.push(refresh_token.len());
        Ok(
            match state
                .link_scenarios
                .pop_front()
                .expect("a link scenario fixture")
            {
                LinkScenario::Fresh => FusionLinkChange::Fresh,
                LinkScenario::Reconnect(previous_refresh_token) => FusionLinkChange::Replaced {
                    previous_refresh_token: MicrosoftRefreshToken::new(previous_refresh_token),
                },
            },
        )
    }

    async fn encrypt_grant(
        &self,
        link_owner_id: &str,
        email: &str,
        _refresh_token: MicrosoftRefreshToken,
    ) -> Result<EncryptedMicrosoftToken, GrantEncryptionFailed> {
        let mut state = self.state.lock().unwrap();
        state.events.push("encrypt");
        state
            .encryption_identities
            .push((link_owner_id.to_owned(), email.to_owned()));
        if self.failure == Failure::Encryption {
            return Err(GrantEncryptionFailed);
        }
        state.encryption_count += 1;

        Ok(EncryptedMicrosoftToken {
            refresh_token_ciphertext: vec![state.encryption_count],
            encrypted_data_key: vec![10, state.encryption_count],
            nonce: vec![state.encryption_count; 12],
            encryption_version: 1,
            kms_key_id: "fake-kms-key".into(),
        })
    }

    async fn persist_grant(
        &self,
        link_owner_id: &str,
        email: &str,
        encrypted_token: &EncryptedMicrosoftToken,
    ) -> Result<(), GrantPersistenceFailed> {
        let mut state = self.state.lock().unwrap();
        state.events.push("persist");
        if self.failure == Failure::Persistence {
            return Err(GrantPersistenceFailed);
        }
        state.grants.insert(
            (link_owner_id.to_owned(), email.to_owned()),
            encrypted_token.refresh_token_ciphertext.clone(),
        );
        Ok(())
    }

    async fn mark_link_consumable(
        &self,
        _link_id: &Uuid,
        email: &str,
    ) -> MicrosoftCallbackResult<()> {
        let mut state = self.state.lock().unwrap();
        state.events.push("mark");
        state.marked_emails.push(email.to_owned());
        Ok(())
    }

    async fn compensate_link(
        &self,
        _identity_provider_id: &str,
        _link_owner_id: &str,
        _identity: &MicrosoftLinkIdentity,
        change: FusionLinkChange,
    ) {
        let compensation = match change {
            FusionLinkChange::Fresh => Compensation::Fresh,
            FusionLinkChange::Replaced { .. } => Compensation::Replaced,
            FusionLinkChange::Unchanged => Compensation::Unchanged,
        };
        self.state.lock().unwrap().compensations.push(compensation);
    }

    async fn cleanup_pending_link(&self, link_id: &Uuid) {
        self.state.lock().unwrap().cleaned_links.push(*link_id);
    }
}
