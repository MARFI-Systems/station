use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Provider identity persisted for one user-owned delegated Microsoft mailbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicrosoftMailbox {
    pub link_id: Uuid,
    pub tenant_id: String,
    pub mailbox_id: String,
    pub user_principal_name: String,
    pub disconnected_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Identity returned by the authorized Microsoft connection flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMicrosoftMailbox {
    pub link_id: Uuid,
    pub tenant_id: String,
    pub mailbox_id: String,
    pub user_principal_name: String,
}

/// Durable per-folder Graph delta state.
///
/// `folder_id` is the stable Graph folder identity. `well_known_name` is only a
/// semantic mapping hint (Inbox, Sent Items, Drafts, Deleted Items, Junk); it is
/// never used as identity. Moves and deletes are reconciled locally by the sync
/// worker, and this state never authorizes a write back to Graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicrosoftFolderSync {
    pub link_id: Uuid,
    pub folder_id: String,
    pub parent_folder_id: Option<String>,
    pub display_name: String,
    pub well_known_name: Option<String>,
    pub delta_cursor: Option<String>,
    pub cursor_generation: i64,
    pub initial_sync_complete: bool,
    pub cursor_invalidated_at: Option<DateTime<Utc>>,
    pub last_successful_sync_at: Option<DateTime<Utc>>,
    pub is_deleted: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Folder metadata discovered during bounded Graph traversal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredMicrosoftFolder {
    pub link_id: Uuid,
    pub folder_id: String,
    pub parent_folder_id: Option<String>,
    pub display_name: String,
    pub well_known_name: Option<String>,
}
