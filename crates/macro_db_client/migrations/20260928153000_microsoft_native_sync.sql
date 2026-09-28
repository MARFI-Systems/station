ALTER TABLE email_microsoft_mailboxes
    ADD COLUMN IF NOT EXISTS folder_walk_cursor JSONB,
    ADD COLUMN IF NOT EXISTS folder_walk_generation BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS folder_walk_seen_ids TEXT[] NOT NULL DEFAULT '{}',
    ADD COLUMN IF NOT EXISTS folder_walk_started_at TIMESTAMPTZ;

CREATE TABLE IF NOT EXISTS email_microsoft_message_state (
    link_id UUID NOT NULL REFERENCES email_links(id) ON DELETE CASCADE,
    provider_message_id TEXT NOT NULL,
    message_id UUID NOT NULL REFERENCES email_messages(id) ON DELETE CASCADE,
    folder_id TEXT NOT NULL,
    change_key TEXT,
    provider_thread_id TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (link_id, provider_message_id)
);

CREATE INDEX IF NOT EXISTS idx_email_microsoft_message_state_folder
    ON email_microsoft_message_state(link_id, folder_id);
