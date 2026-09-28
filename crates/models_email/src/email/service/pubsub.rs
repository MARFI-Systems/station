use crate::service::contact::Contact;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};
use thiserror::Error;
use uuid::Uuid;

/// A worker failure with its queue-routing classification.
///
/// Style exception (CS-46): the email worker error plumbing stays on
/// `anyhow::Error` rather than `rootcause`. Every queue operation, policy
/// module, and DLQ path across email_service composes through this
/// `source` field, so migrating is an all-or-nothing cross-crate change —
/// recorded here as the accepted exception rather than mixing the two
/// error stacks module by module.
#[derive(Debug, Error)]
#[error("{reason}: {source}")]
pub struct DetailedError {
    pub reason: FailureReason,
    #[source]
    pub source: anyhow::Error,
}

#[derive(Debug, Error)]
pub enum ProcessingError {
    #[error("Retryable error occurred")]
    Retryable(#[source] DetailedError),

    #[error("Non-retryable error occurred")]
    NonRetryable(#[source] DetailedError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Display, EnumString)]
pub enum FailureReason {
    DatabaseQueryFailed,
    RedisQueryFailed,
    SqsEnqueueFailed,
    AccessTokenFetchFailed,
    MessageNotFoundInProvider,
    MessageNotFoundInDatabase,
    LinkNotFound,
    BackfillJobNotFound,
    EmailBackfillInitBusy,
    CalendarBackfillBusy,
    GmailApiFailed,
    GmailApiRateLimited,
    OutdatedHistoryId,
    AttachmentParsingFailed,
    DSSUploadFailed,
    InvalidData,
}

/// The reason a link was deleted, stored in the email_links_history table.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum DeletionReason {
    Unused,
    Inactive,
    ManuallyDisabled,
    UserDeleted,
    AccessRevoked,
}

impl DeletionReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            DeletionReason::Unused => "Unused",
            DeletionReason::Inactive => "Inactive",
            DeletionReason::ManuallyDisabled => "ManuallyDisabled",
            DeletionReason::UserDeleted => "UserDeleted",
            DeletionReason::AccessRevoked => "AccessRevoked",
        }
    }
}

/// The message sent to the link manager SQS queue.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "operation")]
pub enum LinkManagerMessage {
    /// Triggers a contact sync and refreshes the Gmail watch subscription to continue receiving
    /// inbox notifications for the user.
    Refresh { link_id: Uuid },
    /// Delete a single link from the database.
    DeleteLink {
        link_id: Uuid,
        deletion_reason: DeletionReason,
    },
    /// Delete all links for a user, identified by fusionauth_user_id.
    DeleteUser { fusionauth_user_id: String },
    /// Notify everyone who can act on a link (its owner and any delegates) that
    /// its grant has gone dead and the inbox needs to be reconnected. Enqueued
    /// once, on the edge where a link first transitions into needs-reauth.
    NotifyReauthRequired { link_id: Uuid },
    /// Probe a single link's grant and record its sync health, without renewing the
    /// Gmail watch or syncing contacts. Enqueued by the periodic health poll so a dead
    /// grant surfaces between the daily full refreshes.
    HealthCheck { link_id: Uuid },
    /// Advances one bounded unit of the native read-only Microsoft mailbox sync.
    MicrosoftSync {
        link_id: Uuid,
        sync_operation: MicrosoftSyncOperation,
    },
}

/// Small resumable units used by the Microsoft mailbox worker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum MicrosoftSyncOperation {
    DiscoverFolders,
    PurgeFolder { folder_id: String },
    SyncFolder {
        folder_id: String,
        expected_generation: i64,
        #[serde(default)]
        rebuild: bool,
    },
}

/// The message we send from the email_scheduled_handler lambda to the service via SQS to trigger
/// the sending of a scheduled message
#[derive(Debug, Serialize, Deserialize)]
pub struct ScheduledPubsubMessage {
    pub link_id: Uuid,
    pub message_id: Uuid,
}

/// The message we send to the sfs_uploader telling it what image URL to upload
#[derive(Debug, Serialize, Deserialize)]
pub struct SFSUploaderMessage {
    pub contact: Contact,
}

#[cfg(test)]
mod microsoft_sync_test {
    use super::*;

    #[test]
    fn microsoft_sync_queue_payload_is_explicit_and_resumable() {
        let value = serde_json::to_value(LinkManagerMessage::MicrosoftSync {
            link_id: Uuid::nil(),
            sync_operation: MicrosoftSyncOperation::SyncFolder {
                folder_id: "opaque-folder-id".into(),
                expected_generation: 7,
                rebuild: true,
            },
        })
        .unwrap();
        assert_eq!(value["operation"], "MicrosoftSync");
        assert_eq!(value["sync_operation"]["phase"], "sync_folder");
        assert_eq!(value["sync_operation"]["folder_id"], "opaque-folder-id");
        let round_trip: LinkManagerMessage = serde_json::from_value(value).unwrap();
        assert!(matches!(
            round_trip,
            LinkManagerMessage::MicrosoftSync {
                sync_operation: MicrosoftSyncOperation::SyncFolder {
                    expected_generation: 7,
                    rebuild: true,
                    ..
                },
                ..
            }
        ));
    }
}
