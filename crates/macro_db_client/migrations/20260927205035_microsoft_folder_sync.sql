CREATE TABLE email_microsoft_mailboxes (
    link_id uuid PRIMARY KEY REFERENCES email_links(id) ON DELETE CASCADE,
    tenant_id text NOT NULL,
    mailbox_id text NOT NULL,
    user_principal_name character varying(320) NOT NULL,
    disconnected_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT email_microsoft_mailboxes_tenant_not_empty CHECK (tenant_id <> ''),
    CONSTRAINT email_microsoft_mailboxes_mailbox_not_empty CHECK (mailbox_id <> ''),
    CONSTRAINT email_microsoft_mailboxes_upn_not_empty CHECK (user_principal_name <> ''),
    CONSTRAINT email_microsoft_mailboxes_upn_lowercase CHECK (
        user_principal_name = lower(user_principal_name)
    )
);

-- Initial scope permits one active delegated Microsoft mailbox per Station user.
-- Gmail rows remain independent and are never replaced by this constraint.
CREATE UNIQUE INDEX email_links_one_active_microsoft_mailbox_per_user
    ON email_links (fusionauth_user_id)
    WHERE provider = 'MICROSOFT' AND is_sync_active = true;

-- Prevent the same live Graph mailbox from being owned by two Station links.
CREATE UNIQUE INDEX email_microsoft_mailboxes_one_active_owner
    ON email_microsoft_mailboxes (tenant_id, mailbox_id)
    WHERE disconnected_at IS NULL;

CREATE TABLE email_microsoft_folder_sync (
    link_id uuid NOT NULL REFERENCES email_microsoft_mailboxes(link_id) ON DELETE CASCADE,
    folder_id text NOT NULL,
    parent_folder_id text,
    display_name character varying(255) NOT NULL,
    well_known_name text,
    delta_cursor text,
    cursor_generation bigint NOT NULL DEFAULT 0,
    initial_sync_complete boolean NOT NULL DEFAULT false,
    cursor_invalidated_at timestamptz,
    last_successful_sync_at timestamptz,
    is_deleted boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (link_id, folder_id),
    CONSTRAINT email_microsoft_folder_sync_folder_not_empty CHECK (folder_id <> ''),
    CONSTRAINT email_microsoft_folder_sync_display_not_empty CHECK (display_name <> ''),
    CONSTRAINT email_microsoft_folder_sync_generation_nonnegative CHECK (cursor_generation >= 0)
);

CREATE INDEX email_microsoft_folder_sync_pending
    ON email_microsoft_folder_sync (link_id, initial_sync_complete, updated_at)
    WHERE is_deleted = false;
