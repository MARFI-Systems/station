use std::collections::VecDeque;
use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::models::EmailApiError;

use super::invalid_response;

/// Opaque continuation for a bounded folder-listing traversal.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicrosoftGraphPageCursor(pub(super) String);

impl MicrosoftGraphPageCursor {
    /// Returns the opaque URL that must be persisted without modification.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for MicrosoftGraphPageCursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MicrosoftGraphPageCursor([REDACTED])")
    }
}

/// A bounded page group of Graph mail folders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicrosoftGraphFolderPage {
    /// Folders read in this bounded call.
    pub folders: Vec<MicrosoftGraphMailFolder>,
    /// Continuation to resume the same direct-child listing, if more pages remain.
    pub next_page: Option<MicrosoftGraphPageCursor>,
    /// Number of provider pages consumed by this call.
    pub pages_fetched: u16,
}

/// One folder discovered by a recursive mailbox walk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicrosoftGraphFolderWalkEntry {
    /// Typed Graph folder data.
    pub folder: MicrosoftGraphMailFolder,
    /// Display-name path from the mailbox root to this folder.
    ///
    /// Components are kept separate because folder names may contain `/`.
    pub path: Vec<String>,
}

/// A bounded step of a resumable recursive mailbox folder walk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicrosoftGraphFolderWalkPage {
    /// Folders discovered while processing the current parent collection.
    pub folders: Vec<MicrosoftGraphFolderWalkEntry>,
    /// Cursor for the next bounded step, or `None` when traversal is complete.
    pub next_cursor: Option<MicrosoftGraphFolderWalkCursor>,
    /// Number of provider pages consumed by this step.
    pub pages_fetched: u16,
}

/// Durable state for a bounded recursive folder walk.
///
/// The cursor belongs to one delegated `/me` mailbox. Account ownership and
/// durable cross-user isolation remain responsibilities of the integration
/// layer that stores it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicrosoftGraphFolderWalkCursor {
    pub(super) current_parent_id: Option<String>,
    pub(super) current_parent_path: Vec<String>,
    pub(super) current_page: Option<MicrosoftGraphPageCursor>,
    pub(super) pending_parents: VecDeque<PendingFolderWalkParent>,
    pub(super) discovered_folder_ids: Vec<String>,
}

impl MicrosoftGraphFolderWalkCursor {
    pub(super) fn root() -> Self {
        Self {
            current_parent_id: None,
            current_parent_path: Vec::new(),
            current_page: None,
            pending_parents: VecDeque::new(),
            discovered_folder_ids: Vec::new(),
        }
    }

    /// Returns the number of child-folder collections queued after the current parent.
    pub fn pending_parent_count(&self) -> usize {
        self.pending_parents.len()
    }
}

impl fmt::Debug for MicrosoftGraphFolderWalkCursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MicrosoftGraphFolderWalkCursor")
            .field("current_parent_id", &self.current_parent_id)
            .field("current_parent_path", &self.current_parent_path)
            .field("current_page", &self.current_page)
            .field("pending_parent_count", &self.pending_parents.len())
            .field("discovered_folder_count", &self.discovered_folder_ids.len())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct PendingFolderWalkParent {
    pub(super) folder_id: String,
    pub(super) path: Vec<String>,
}

/// Result of resuming a stored per-folder delta cursor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MicrosoftGraphFolderDeltaResume {
    /// The supplied cursor remained valid and the returned batch continues it.
    Resumed(MicrosoftGraphMessageDeltaBatch),
    /// Graph expired the supplied cursor and the returned batch restarted from current state.
    ///
    /// Callers must rebuild that folder's local projection rather than apply
    /// this batch as an ordinary incremental update, because messages deleted
    /// before the new snapshot cannot appear as tombstones. If this bounded
    /// batch ends with a continuation cursor, rebuild mode must remain active
    /// until the replacement delta round completes.
    RestartedAfterExpiredCursor(MicrosoftGraphMessageDeltaBatch),
}

impl MicrosoftGraphFolderDeltaResume {
    /// Borrows the bounded delta batch produced by the operation.
    pub fn batch(&self) -> &MicrosoftGraphMessageDeltaBatch {
        match self {
            Self::Resumed(batch) | Self::RestartedAfterExpiredCursor(batch) => batch,
        }
    }

    /// Returns whether the caller must rebuild the folder projection.
    pub fn requires_folder_rebuild(&self) -> bool {
        matches!(self, Self::RestartedAfterExpiredCursor(_))
    }
}

/// A Microsoft Graph mail folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrosoftGraphMailFolder {
    /// Provider folder identifier.
    pub id: String,
    /// User-visible folder name.
    pub display_name: String,
    /// Parent provider folder identifier.
    pub parent_folder_id: Option<String>,
    /// Direct child-folder count reported by Graph.
    pub child_folder_count: u64,
    /// Unread item count reported by Graph.
    pub unread_item_count: u64,
    /// Total item count reported by Graph.
    pub total_item_count: u64,
    /// Whether Graph marks the folder hidden.
    pub is_hidden: bool,
}

/// Whether a Graph delta cursor continues a round or starts the next round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MicrosoftGraphDeltaCursorKind {
    /// More pages remain in the current delta round.
    Continuation,
    /// The current round is complete; this cursor starts the next round.
    Delta,
}

/// Opaque, per-folder Microsoft Graph delta cursor.
///
/// The full URL is intentionally retained because Graph embeds query shape and
/// state in it. The client validates the trusted origin and exact folder delta
/// path every time before attaching a bearer token.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicrosoftGraphDeltaCursor {
    kind: MicrosoftGraphDeltaCursorKind,
    url: String,
}

impl MicrosoftGraphDeltaCursor {
    pub(super) fn new(kind: MicrosoftGraphDeltaCursorKind, url: String) -> Self {
        Self { kind, url }
    }

    /// Returns whether this cursor continues or completes a delta round.
    pub fn kind(&self) -> MicrosoftGraphDeltaCursorKind {
        self.kind
    }

    /// Returns the opaque URL that must be persisted without modification.
    pub fn as_str(&self) -> &str {
        &self.url
    }
}

impl fmt::Debug for MicrosoftGraphDeltaCursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MicrosoftGraphDeltaCursor")
            .field("kind", &self.kind)
            .field("url", &"[REDACTED]")
            .finish()
    }
}

/// A bounded group of per-folder message changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MicrosoftGraphMessageDeltaBatch {
    /// Message upserts and tombstones, in provider order.
    pub changes: Vec<MicrosoftGraphMessageDeltaChange>,
    /// Cursor to persist after accepting this batch.
    pub cursor: MicrosoftGraphDeltaCursor,
    /// Number of provider pages consumed by this call.
    pub pages_fetched: u16,
}

/// One message change returned by a folder delta round.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MicrosoftGraphMessageDeltaChange {
    /// A message was created or updated in this folder.
    Upsert(MicrosoftGraphMessage),
    /// A message was deleted or moved out of this folder.
    Removed(MicrosoftGraphRemovedMessage),
}

/// A Graph message tombstone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicrosoftGraphRemovedMessage {
    /// Immutable message identifier returned by Graph.
    pub id: String,
    /// Provider removal reason, normally `deleted` for deletes and moves.
    pub reason: String,
}

/// A read-only Microsoft Graph message projection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MicrosoftGraphMessage {
    /// Immutable message identifier.
    pub id: String,
    /// Provider change key.
    pub change_key: Option<String>,
    /// Graph conversation identifier; no thread-count semantics are implied.
    pub conversation_id: Option<String>,
    /// Current parent folder identifier.
    pub parent_folder_id: Option<String>,
    /// RFC message identifier when present.
    pub internet_message_id: Option<String>,
    /// Message subject.
    pub subject: Option<String>,
    /// Provider-generated body preview.
    pub body_preview: Option<String>,
    /// Full untrusted message body when requested by
    /// [`MicrosoftGraphMailClient::get_message`](super::MicrosoftGraphMailClient::get_message).
    pub body: Option<MicrosoftGraphItemBody>,
    /// Author represented in the From header.
    pub from: Option<MicrosoftGraphEmailAddress>,
    /// Account that actually submitted the message.
    pub sender: Option<MicrosoftGraphEmailAddress>,
    /// To recipients.
    pub to_recipients: Vec<MicrosoftGraphEmailAddress>,
    /// Cc recipients.
    pub cc_recipients: Vec<MicrosoftGraphEmailAddress>,
    /// Bcc recipients.
    pub bcc_recipients: Vec<MicrosoftGraphEmailAddress>,
    /// Received timestamp.
    pub received_at: Option<DateTime<Utc>>,
    /// Sent timestamp.
    pub sent_at: Option<DateTime<Utc>>,
    /// Whether Graph reports the message read.
    pub is_read: bool,
    /// Whether Graph reports the message as a draft.
    pub is_draft: bool,
    /// Whether Graph reports one or more attachments.
    pub has_attachments: bool,
    /// Provider importance.
    pub importance: MicrosoftGraphImportance,
    /// Outlook category names.
    pub categories: Vec<String>,
    /// Attachment metadata included by message retrieval.
    pub attachments: Vec<MicrosoftGraphAttachment>,
}

/// A Graph message body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicrosoftGraphItemBody {
    /// Provider content type.
    pub content_type: MicrosoftGraphBodyContentType,
    /// Raw, untrusted provider content.
    pub content: String,
}

/// Content type reported for a Graph message body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MicrosoftGraphBodyContentType {
    /// Plain text body.
    Text,
    /// HTML body.
    Html,
    /// A provider value not known to this client version.
    Other(String),
}

/// Graph message importance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MicrosoftGraphImportance {
    /// Low importance.
    Low,
    /// Normal importance.
    Normal,
    /// High importance.
    High,
    /// A provider value not known to this client version.
    Other(String),
}

/// A Graph email address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicrosoftGraphEmailAddress {
    /// Display name when supplied.
    pub name: Option<String>,
    /// Email address.
    pub address: String,
}

/// Metadata for a Graph message attachment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicrosoftGraphAttachment {
    /// Immutable attachment identifier.
    pub id: String,
    /// Attachment kind.
    pub kind: MicrosoftGraphAttachmentKind,
    /// Original filename or item name.
    pub name: Option<String>,
    /// Declared content type.
    pub content_type: Option<String>,
    /// Provider-reported size in bytes.
    pub size_bytes: Option<u64>,
    /// Whether the attachment is inline.
    pub is_inline: bool,
    /// Content ID for inline references.
    pub content_id: Option<String>,
}

/// Graph attachment resource kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MicrosoftGraphAttachmentKind {
    /// File attachment.
    File,
    /// Outlook item attachment.
    Item,
    /// Cloud reference attachment.
    Reference,
    /// A provider value not known to this client version.
    Other(String),
}

#[derive(Debug, Deserialize)]
pub(super) struct FolderCollectionWire {
    pub(super) value: Vec<MicrosoftGraphMailFolder>,
    #[serde(rename = "@odata.nextLink")]
    pub(super) next_link: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MessageDeltaWire {
    pub(super) value: Vec<MessageWire>,
    #[serde(rename = "@odata.nextLink")]
    pub(super) next_link: Option<String>,
    #[serde(rename = "@odata.deltaLink")]
    pub(super) delta_link: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MessageWire {
    id: String,
    change_key: Option<String>,
    conversation_id: Option<String>,
    parent_folder_id: Option<String>,
    internet_message_id: Option<String>,
    subject: Option<String>,
    body_preview: Option<String>,
    body: Option<ItemBodyWire>,
    from: Option<RecipientWire>,
    sender: Option<RecipientWire>,
    #[serde(default)]
    to_recipients: Vec<RecipientWire>,
    #[serde(default)]
    cc_recipients: Vec<RecipientWire>,
    #[serde(default)]
    bcc_recipients: Vec<RecipientWire>,
    received_date_time: Option<DateTime<Utc>>,
    sent_date_time: Option<DateTime<Utc>>,
    #[serde(default)]
    is_read: bool,
    #[serde(default)]
    is_draft: bool,
    #[serde(default)]
    has_attachments: bool,
    importance: Option<String>,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    attachments: Vec<AttachmentWire>,
    #[serde(rename = "@removed")]
    removed: Option<RemovedWire>,
}

#[derive(Debug, Deserialize)]
struct RemovedWire {
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemBodyWire {
    content_type: String,
    content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecipientWire {
    email_address: EmailAddressWire,
}

#[derive(Debug, Deserialize)]
struct EmailAddressWire {
    name: Option<String>,
    address: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachmentWire {
    id: String,
    #[serde(rename = "@odata.type")]
    odata_type: Option<String>,
    name: Option<String>,
    content_type: Option<String>,
    size: Option<u64>,
    #[serde(default)]
    is_inline: bool,
    content_id: Option<String>,
}

impl TryFrom<MessageWire> for MicrosoftGraphMessageDeltaChange {
    type Error = EmailApiError;

    fn try_from(message: MessageWire) -> Result<Self, Self::Error> {
        if let Some(reason) = message
            .removed
            .as_ref()
            .map(|removed| removed.reason.clone())
        {
            return Ok(Self::Removed(MicrosoftGraphRemovedMessage {
                id: message.id,
                reason,
            }));
        }
        Ok(Self::Upsert(message.try_into()?))
    }
}

impl TryFrom<MessageWire> for MicrosoftGraphMessage {
    type Error = EmailApiError;

    fn try_from(message: MessageWire) -> Result<Self, Self::Error> {
        if message.removed.is_some() {
            return Err(invalid_response(
                "message retrieval returned a delta tombstone",
            ));
        }
        Ok(Self {
            id: message.id,
            change_key: message.change_key,
            conversation_id: message.conversation_id,
            parent_folder_id: message.parent_folder_id,
            internet_message_id: message.internet_message_id,
            subject: message.subject,
            body_preview: message.body_preview,
            body: message.body.map(Into::into),
            from: message.from.map(Into::into),
            sender: message.sender.map(Into::into),
            to_recipients: message.to_recipients.into_iter().map(Into::into).collect(),
            cc_recipients: message.cc_recipients.into_iter().map(Into::into).collect(),
            bcc_recipients: message.bcc_recipients.into_iter().map(Into::into).collect(),
            received_at: message.received_date_time,
            sent_at: message.sent_date_time,
            is_read: message.is_read,
            is_draft: message.is_draft,
            has_attachments: message.has_attachments,
            importance: MicrosoftGraphImportance::from_provider(message.importance),
            categories: message.categories,
            attachments: message.attachments.into_iter().map(Into::into).collect(),
        })
    }
}

impl From<ItemBodyWire> for MicrosoftGraphItemBody {
    fn from(body: ItemBodyWire) -> Self {
        Self {
            content_type: MicrosoftGraphBodyContentType::from_provider(body.content_type),
            content: body.content,
        }
    }
}

impl From<RecipientWire> for MicrosoftGraphEmailAddress {
    fn from(recipient: RecipientWire) -> Self {
        Self {
            name: recipient.email_address.name,
            address: recipient.email_address.address,
        }
    }
}

impl From<AttachmentWire> for MicrosoftGraphAttachment {
    fn from(attachment: AttachmentWire) -> Self {
        Self {
            id: attachment.id,
            kind: MicrosoftGraphAttachmentKind::from_provider(attachment.odata_type),
            name: attachment.name,
            content_type: attachment.content_type,
            size_bytes: attachment.size,
            is_inline: attachment.is_inline,
            content_id: attachment.content_id,
        }
    }
}

impl MicrosoftGraphBodyContentType {
    fn from_provider(value: String) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "text" => Self::Text,
            "html" => Self::Html,
            _ => Self::Other(value),
        }
    }
}

impl MicrosoftGraphImportance {
    fn from_provider(value: Option<String>) -> Self {
        match value.as_deref().map(str::to_ascii_lowercase).as_deref() {
            Some("low") => Self::Low,
            Some("normal") | None => Self::Normal,
            Some("high") => Self::High,
            Some(_) => Self::Other(value.expect("matched a present provider value")),
        }
    }
}

impl MicrosoftGraphAttachmentKind {
    fn from_provider(value: Option<String>) -> Self {
        match value.as_deref() {
            Some("#microsoft.graph.fileAttachment") => Self::File,
            Some("#microsoft.graph.itemAttachment") => Self::Item,
            Some("#microsoft.graph.referenceAttachment") => Self::Reference,
            Some(_) => Self::Other(value.expect("matched a present provider value")),
            None => Self::Other("unknown".to_string()),
        }
    }
}
