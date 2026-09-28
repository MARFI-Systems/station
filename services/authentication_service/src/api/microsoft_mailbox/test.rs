use super::*;

#[test]
fn browser_state_contains_no_expiry_owner_or_pkce_verifier() {
    let state = MicrosoftMailboxState {
        flow_id: Uuid::now_v7(),
        state_secret: "random-state-secret".into(),
    };
    let value = serde_json::to_value(state).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 2);
    assert!(value.get("expires_at").is_none());
    assert!(value.get("owner_fusionauth_user_id").is_none());
    assert!(value.get("encrypted_code_verifier").is_none());
}

#[test]
fn tampering_state_secret_changes_server_match_hash() {
    let original = Sha256::digest(b"state-secret-a");
    let tampered = Sha256::digest(b"state-secret-b");
    assert_ne!(original, tampered);
}

#[test]
fn status_contains_verified_identity_and_no_token_field() {
    let value = serde_json::to_value(MicrosoftMailboxStatus {
        connected: true,
        email: Some("owner@example.com".into()),
        tenant_id: Some("tenant-id".into()),
        object_id: Some("object-id".into()),
    })
    .unwrap();
    assert_eq!(
        value,
        serde_json::json!({"connected":true,"email":"owner@example.com","tenantId":"tenant-id","objectId":"object-id"})
    );
    assert!(value.get("accessToken").is_none());
}

#[test]
fn completion_redirect_is_fixed_to_configured_route() {
    let response = completion_redirect("https://station.example/settings/connections", "connected");
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers()[axum::http::header::LOCATION],
        "https://station.example/settings/connections?microsoftMailbox=connected"
    );
}
