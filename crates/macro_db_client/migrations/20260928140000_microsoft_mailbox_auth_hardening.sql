-- Existing rows predate purpose and verified Entra identity metadata. Label them
-- legacy and fail closed in application queries rather than assuming mailbox consent.
ALTER TABLE microsoft_oauth_grants
    ADD COLUMN grant_purpose text NOT NULL DEFAULT 'legacy_identity',
    ADD COLUMN grant_schema_version smallint NOT NULL DEFAULT 0,
    ADD COLUMN microsoft_tenant_id text,
    ADD COLUMN microsoft_object_id text,
    ADD COLUMN disconnected_at timestamptz,
    ADD CONSTRAINT microsoft_oauth_grants_purpose_not_empty CHECK (grant_purpose <> ''),
    ADD CONSTRAINT microsoft_oauth_grants_schema_version_nonnegative CHECK (grant_schema_version >= 0),
    ADD CONSTRAINT microsoft_oauth_grants_mailbox_identity_complete CHECK (
        grant_purpose <> 'mailbox_read'
        OR (
            grant_schema_version = 1
            AND microsoft_tenant_id IS NOT NULL
            AND microsoft_tenant_id <> ''
            AND microsoft_object_id IS NOT NULL
            AND microsoft_object_id <> ''
        )
    );

CREATE UNIQUE INDEX microsoft_oauth_grants_one_active_mailbox_per_user
    ON microsoft_oauth_grants (fusionauth_user_id)
    WHERE grant_purpose = 'mailbox_read' AND disconnected_at IS NULL;

CREATE UNIQUE INDEX microsoft_oauth_grants_one_active_owner_per_entra_mailbox
    ON microsoft_oauth_grants (microsoft_tenant_id, microsoft_object_id)
    WHERE grant_purpose = 'mailbox_read' AND disconnected_at IS NULL;

-- PKCE verifiers never leave the service in plaintext. The browser receives
-- only a flow id plus random state secret; its SHA-256 hash is matched here.
CREATE TABLE microsoft_mailbox_oauth_flows (
    flow_id uuid PRIMARY KEY,
    fusionauth_user_id text NOT NULL,
    state_hash bytea NOT NULL,
    verifier_ciphertext bytea NOT NULL,
    encrypted_data_key bytea NOT NULL,
    nonce bytea NOT NULL,
    encryption_version integer NOT NULL,
    kms_key_id text NOT NULL,
    expires_at timestamptz NOT NULL,
    consumed_at timestamptz,
    canceled_at timestamptz,
    completed_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT microsoft_mailbox_oauth_flows_owner_not_empty CHECK (fusionauth_user_id <> ''),
    CONSTRAINT microsoft_mailbox_oauth_flows_state_hash_sha256 CHECK (octet_length(state_hash) = 32),
    CONSTRAINT microsoft_mailbox_oauth_flows_ciphertext_not_empty CHECK (octet_length(verifier_ciphertext) > 0),
    CONSTRAINT microsoft_mailbox_oauth_flows_data_key_not_empty CHECK (octet_length(encrypted_data_key) > 0),
    CONSTRAINT microsoft_mailbox_oauth_flows_nonce_length CHECK (octet_length(nonce) = 12),
    CONSTRAINT microsoft_mailbox_oauth_flows_encryption_version_positive CHECK (encryption_version > 0),
    CONSTRAINT microsoft_mailbox_oauth_flows_kms_key_not_empty CHECK (kms_key_id <> ''),
    CONSTRAINT microsoft_mailbox_oauth_flows_expiry_after_creation CHECK (expires_at > created_at),
    CONSTRAINT microsoft_mailbox_oauth_flows_expiry_bounded CHECK (expires_at <= created_at + interval '10 minutes'),
    CONSTRAINT microsoft_mailbox_oauth_flows_consumed_after_creation CHECK (consumed_at IS NULL OR consumed_at >= created_at),
    CONSTRAINT microsoft_mailbox_oauth_flows_canceled_after_creation CHECK (canceled_at IS NULL OR canceled_at >= created_at),
    CONSTRAINT microsoft_mailbox_oauth_flows_completed_after_consumption CHECK (
        completed_at IS NULL OR (consumed_at IS NOT NULL AND completed_at >= consumed_at)
    ),
    CONSTRAINT microsoft_mailbox_oauth_flows_not_completed_when_canceled CHECK (
        canceled_at IS NULL OR completed_at IS NULL
    )
);

CREATE INDEX microsoft_mailbox_oauth_flows_owner_cleanup
    ON microsoft_mailbox_oauth_flows (fusionauth_user_id, expires_at);
