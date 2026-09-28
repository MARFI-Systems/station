//! Read-only Microsoft Graph mail adapter.
//!
//! This adapter deliberately exposes Graph-specific folder and delta cursors.
//! Gmail's global history and thread-oriented synchronization contracts cannot
//! honestly represent Graph's per-folder delta protocol.
//!
//! The resumable folder-walk and one-shot expired-cursor recovery design was
//! informed by OpenArchiver's AGPL-3.0 Graph connectors. See the crate's
//! `THIRD-PARTY_NOTICES.md` for pinned provenance; no Inbox Zero code is used.

use std::collections::HashSet;
use std::fmt;
use std::num::{NonZeroU16, NonZeroUsize};
use std::time::{Duration, SystemTime};

use reqwest::header::{ACCEPT, CONTENT_LENGTH, PREFER, RETRY_AFTER};
use reqwest::{Response, StatusCode};
use serde::de::DeserializeOwned;
use thiserror::Error;
use url::Url;

use crate::domain::models::{AccessToken, EmailApiError, RateLimitOrigin};
use crate::domain::ports::MailboxAttachmentClient;

const GRAPH_V1_SEGMENT: &str = "v1.0";
const DEFAULT_PAGE_SIZE: u16 = 100;
const MAX_PAGE_SIZE: u16 = 1_000;
const DEFAULT_MAX_PAGES_PER_CALL: u16 = 5;
const DEFAULT_MAX_ATTACHMENT_BYTES: usize = 25 * 1024 * 1024;
const DEFAULT_MAX_FOLDER_WALK_FOLDERS: usize = 10_000;
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_JSON_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MESSAGE_SELECT: &str = "id,changeKey,conversationId,parentFolderId,internetMessageId,subject,bodyPreview,body,from,sender,toRecipients,ccRecipients,bccRecipients,receivedDateTime,sentDateTime,isRead,isDraft,hasAttachments,importance,categories,attachments";
const DELTA_MESSAGE_SELECT: &str = "id,changeKey,conversationId,parentFolderId,internetMessageId,subject,bodyPreview,from,sender,toRecipients,ccRecipients,bccRecipients,receivedDateTime,sentDateTime,isRead,isDraft,hasAttachments,importance,categories";
const FOLDER_SELECT: &str =
    "id,displayName,parentFolderId,childFolderCount,unreadItemCount,totalItemCount,isHidden";

/// Configuration for [`MicrosoftGraphMailClient`].
#[derive(Clone, Debug)]
pub struct MicrosoftGraphMailClientConfig {
    trusted_origin: Url,
    page_size: NonZeroU16,
    max_pages_per_call: NonZeroU16,
    max_attachment_bytes: NonZeroUsize,
    max_folder_walk_folders: NonZeroUsize,
    request_timeout: Duration,
}

impl Default for MicrosoftGraphMailClientConfig {
    fn default() -> Self {
        Self {
            trusted_origin: Url::parse("https://graph.microsoft.com/")
                .expect("the built-in Microsoft Graph origin is valid"),
            page_size: NonZeroU16::new(DEFAULT_PAGE_SIZE).expect("default page size is non-zero"),
            max_pages_per_call: NonZeroU16::new(DEFAULT_MAX_PAGES_PER_CALL)
                .expect("default page bound is non-zero"),
            max_attachment_bytes: NonZeroUsize::new(DEFAULT_MAX_ATTACHMENT_BYTES)
                .expect("default attachment bound is non-zero"),
            max_folder_walk_folders: NonZeroUsize::new(DEFAULT_MAX_FOLDER_WALK_FOLDERS)
                .expect("default folder walk bound is non-zero"),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }
}

impl MicrosoftGraphMailClientConfig {
    /// Creates configuration for a trusted Graph origin.
    ///
    /// Production callers should normally use [`Default`]. A custom origin is
    /// useful for a trusted proxy or a synthetic test server. The origin must
    /// not contain credentials, query parameters, fragments, or a non-root path.
    pub fn for_origin(trusted_origin: Url) -> Self {
        Self {
            trusted_origin,
            ..Self::default()
        }
    }

    /// Sets the requested provider page size.
    pub fn with_page_size(mut self, page_size: NonZeroU16) -> Self {
        self.page_size = page_size;
        self
    }

    /// Sets the maximum number of provider pages fetched by one method call.
    pub fn with_max_pages_per_call(mut self, max_pages: NonZeroU16) -> Self {
        self.max_pages_per_call = max_pages;
        self
    }

    /// Sets the largest attachment body accepted by the client.
    pub fn with_max_attachment_bytes(mut self, max_bytes: NonZeroUsize) -> Self {
        self.max_attachment_bytes = max_bytes;
        self
    }

    /// Sets the largest number of unique folders retained by a recursive walk cursor.
    pub fn with_max_folder_walk_folders(mut self, max_folders: NonZeroUsize) -> Self {
        self.max_folder_walk_folders = max_folders;
        self
    }

    /// Sets the timeout applied to each provider request.
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }
}

/// Invalid Microsoft Graph client configuration.
#[derive(Debug, Error)]
pub enum MicrosoftGraphConfigError {
    /// The trusted origin is not a bare HTTPS origin or loopback HTTP origin.
    #[error("invalid Microsoft Graph trusted origin: {0}")]
    InvalidOrigin(String),
    /// The configured page size exceeds Microsoft's supported bound.
    #[error("Microsoft Graph page size {0} exceeds the supported maximum")]
    InvalidPageSize(u16),
    /// The request timeout was zero.
    #[error("Microsoft Graph request timeout must be non-zero")]
    ZeroTimeout,
    /// The underlying HTTP client could not be constructed.
    #[error("failed to construct Microsoft Graph HTTP client: {0}")]
    HttpClient(String),
}

/// Read-only Microsoft Graph mail client.
///
/// The public API contains only GET-backed folder, delta, message, and
/// attachment operations. It does not implement sending, label mutation,
/// subscriptions, or Gmail-shaped thread/count synchronization ports.
#[derive(Clone)]
pub struct MicrosoftGraphMailClient {
    client: reqwest::Client,
    trusted_origin: Url,
    page_size: NonZeroU16,
    max_pages_per_call: NonZeroU16,
    max_attachment_bytes: NonZeroUsize,
    max_folder_walk_folders: NonZeroUsize,
}

impl fmt::Debug for MicrosoftGraphMailClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MicrosoftGraphMailClient")
            .field("trusted_origin", &self.trusted_origin)
            .field("page_size", &self.page_size)
            .field("max_pages_per_call", &self.max_pages_per_call)
            .field("max_attachment_bytes", &self.max_attachment_bytes)
            .field("max_folder_walk_folders", &self.max_folder_walk_folders)
            .finish_non_exhaustive()
    }
}

impl MicrosoftGraphMailClient {
    /// Creates a client for the global Microsoft Graph service with bounded defaults.
    pub fn new() -> Result<Self, MicrosoftGraphConfigError> {
        Self::with_config(MicrosoftGraphMailClientConfig::default())
    }

    /// Creates a client from validated configuration.
    pub fn with_config(
        config: MicrosoftGraphMailClientConfig,
    ) -> Result<Self, MicrosoftGraphConfigError> {
        validate_origin(&config.trusted_origin)?;
        if config.page_size.get() > MAX_PAGE_SIZE {
            return Err(MicrosoftGraphConfigError::InvalidPageSize(
                config.page_size.get(),
            ));
        }
        if config.request_timeout.is_zero() {
            return Err(MicrosoftGraphConfigError::ZeroTimeout);
        }

        let client = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| MicrosoftGraphConfigError::HttpClient(error.to_string()))?;

        Ok(Self {
            client,
            trusted_origin: config.trusted_origin,
            page_size: config.page_size,
            max_pages_per_call: config.max_pages_per_call,
            max_attachment_bytes: config.max_attachment_bytes,
            max_folder_walk_folders: config.max_folder_walk_folders,
        })
    }

    /// Lists a bounded number of pages of root folders or direct children.
    ///
    /// Pass `parent_folder_id = None` for folders directly below the mailbox
    /// root. Pass a parent ID to traverse that folder's direct children. Hidden
    /// folders are included so a sync owner can make an explicit policy choice.
    /// If `next_page` is returned, persist it and pass it back with the same
    /// parent folder to continue without an unbounded traversal.
    pub async fn list_mail_folders(
        &self,
        access_token: &AccessToken,
        parent_folder_id: Option<&str>,
        next_page: Option<&MicrosoftGraphPageCursor>,
    ) -> Result<MicrosoftGraphFolderPage, EmailApiError> {
        let mut initial_url = self.folder_collection_url(parent_folder_id);
        let expected_path = initial_url.path().to_owned();
        let page_size = self.page_size.get().to_string();
        initial_url
            .query_pairs_mut()
            .append_pair("includeHiddenFolders", "true")
            .append_pair("$top", &page_size)
            .append_pair("$select", FOLDER_SELECT);

        let mut current_url = match next_page {
            Some(cursor) => self.validated_continuation(cursor.as_str(), &expected_path)?,
            None => initial_url,
        };
        let mut folders = Vec::new();

        for page_index in 0..self.max_pages_per_call.get() {
            let response = self
                .send_get(current_url.clone(), access_token, None)
                .await?;
            let response = successful_response(response).await?;
            let page: FolderCollectionWire = decode_json_limited(response).await?;
            folders.extend(page.value);

            let Some(next_link) = page.next_link else {
                return Ok(MicrosoftGraphFolderPage {
                    folders,
                    next_page: None,
                    pages_fetched: page_index + 1,
                });
            };
            let validated = self.validated_continuation(&next_link, &expected_path)?;
            let cursor = MicrosoftGraphPageCursor(next_link);
            if page_index + 1 == self.max_pages_per_call.get() {
                return Ok(MicrosoftGraphFolderPage {
                    folders,
                    next_page: Some(cursor),
                    pages_fetched: page_index + 1,
                });
            }
            current_url = validated;
        }

        unreachable!("the non-zero page bound always returns from the loop")
    }

    /// Advances a bounded, resumable recursive walk of the delegated mailbox's folder tree.
    ///
    /// Each call consumes at most the configured direct-listing page bound for
    /// one parent collection. Child collections are queued in the returned
    /// cursor, so callers can persist progress without an unbounded recursive
    /// request. Every provider request remains scoped to delegated `/me`.
    pub async fn walk_mail_folders(
        &self,
        access_token: &AccessToken,
        cursor: Option<&MicrosoftGraphFolderWalkCursor>,
    ) -> Result<MicrosoftGraphFolderWalkPage, EmailApiError> {
        let mut state = cursor
            .cloned()
            .unwrap_or_else(MicrosoftGraphFolderWalkCursor::root);
        self.validate_folder_walk_state(&state)?;

        let page = self
            .list_mail_folders(
                access_token,
                state.current_parent_id.as_deref(),
                state.current_page.as_ref(),
            )
            .await?;
        let pages_fetched = page.pages_fetched;
        let mut entries = Vec::with_capacity(page.folders.len());

        for folder in page.folders {
            let mut path = state.current_parent_path.clone();
            path.push(folder.display_name.clone());
            let is_new = !state.discovered_folder_ids.contains(&folder.id);
            if is_new {
                if state.discovered_folder_ids.len() >= self.max_folder_walk_folders.get() {
                    return Err(EmailApiError::Permanent {
                        message: "Microsoft Graph folder walk exceeds the configured folder limit"
                            .to_string(),
                    });
                }
                state.discovered_folder_ids.push(folder.id.clone());
            }
            if is_new && folder.child_folder_count > 0 {
                state.pending_parents.push_back(PendingFolderWalkParent {
                    folder_id: folder.id.clone(),
                    path: path.clone(),
                });
            }
            entries.push(MicrosoftGraphFolderWalkEntry { folder, path });
        }

        if let Some(next_page) = page.next_page {
            state.current_page = Some(next_page);
            return Ok(MicrosoftGraphFolderWalkPage {
                folders: entries,
                next_cursor: Some(state),
                pages_fetched,
            });
        }

        state.current_page = None;
        let next_cursor = match state.pending_parents.pop_front() {
            Some(next_parent) => {
                state.current_parent_id = Some(next_parent.folder_id);
                state.current_parent_path = next_parent.path;
                Some(state)
            }
            None => None,
        };
        Ok(MicrosoftGraphFolderWalkPage {
            folders: entries,
            next_cursor,
            pages_fetched,
        })
    }

    /// Reads a bounded round of per-folder message delta pages.
    ///
    /// A returned continuation cursor means the current round has more pages.
    /// A returned delta cursor means the round is complete and that cursor
    /// should start the next incremental round for the same folder. Removed
    /// entries are retained as tombstones; Graph emits them for both deletes
    /// and moves out of the tracked folder.
    pub async fn list_folder_message_delta(
        &self,
        access_token: &AccessToken,
        folder_id: &str,
        cursor: Option<&MicrosoftGraphDeltaCursor>,
    ) -> Result<MicrosoftGraphMessageDeltaBatch, EmailApiError> {
        let mut initial_url =
            self.graph_url(&["me", "mailFolders", folder_id, "messages", "delta"]);
        let expected_path = initial_url.path().to_owned();
        initial_url
            .query_pairs_mut()
            .append_pair("$select", DELTA_MESSAGE_SELECT);

        let mut current_url = match cursor {
            Some(cursor) => self.validated_continuation(cursor.as_str(), &expected_path)?,
            None => initial_url,
        };
        let prefer = format!(
            "odata.maxpagesize={}, IdType=\"ImmutableId\"",
            self.page_size
        );
        let mut changes = Vec::new();

        for page_index in 0..self.max_pages_per_call.get() {
            let response = self
                .send_get(current_url.clone(), access_token, Some(&prefer))
                .await?;
            let response = successful_response(response).await?;
            let page: MessageDeltaWire = decode_json_limited(response).await?;
            changes.extend(
                page.value
                    .into_iter()
                    .map(MicrosoftGraphMessageDeltaChange::try_from)
                    .collect::<Result<Vec<_>, _>>()?,
            );

            match (page.next_link, page.delta_link) {
                (Some(next_link), None) => {
                    let validated = self.validated_continuation(&next_link, &expected_path)?;
                    let next_cursor = MicrosoftGraphDeltaCursor::new(
                        MicrosoftGraphDeltaCursorKind::Continuation,
                        next_link,
                    );
                    if page_index + 1 == self.max_pages_per_call.get() {
                        return Ok(MicrosoftGraphMessageDeltaBatch {
                            changes,
                            cursor: next_cursor,
                            pages_fetched: page_index + 1,
                        });
                    }
                    current_url = validated;
                }
                (None, Some(delta_link)) => {
                    self.validated_continuation(&delta_link, &expected_path)?;
                    return Ok(MicrosoftGraphMessageDeltaBatch {
                        changes,
                        cursor: MicrosoftGraphDeltaCursor::new(
                            MicrosoftGraphDeltaCursorKind::Delta,
                            delta_link,
                        ),
                        pages_fetched: page_index + 1,
                    });
                }
                _ => {
                    return Err(invalid_response(
                        "message delta page must contain exactly one continuation or delta link",
                    ));
                }
            }
        }

        unreachable!("the non-zero page bound always returns from the loop")
    }

    /// Resumes a stored folder delta cursor and performs one bounded recovery on expiry.
    ///
    /// A Graph 410 from the supplied cursor triggers one fresh delta request for
    /// the same delegated `/me` folder. The typed result forces the integration
    /// layer to distinguish an ordinary incremental batch from a folder snapshot
    /// rebuild. A 410 from that fresh request is returned and is never looped.
    pub async fn resume_folder_message_delta(
        &self,
        access_token: &AccessToken,
        folder_id: &str,
        cursor: &MicrosoftGraphDeltaCursor,
    ) -> Result<MicrosoftGraphFolderDeltaResume, EmailApiError> {
        match self
            .list_folder_message_delta(access_token, folder_id, Some(cursor))
            .await
        {
            Ok(batch) => Ok(MicrosoftGraphFolderDeltaResume::Resumed(batch)),
            Err(EmailApiError::OutdatedCursor) => self
                .list_folder_message_delta(access_token, folder_id, None)
                .await
                .map(MicrosoftGraphFolderDeltaResume::RestartedAfterExpiredCursor),
            Err(error) => Err(error),
        }
    }

    /// Retrieves one Graph message, including attachment metadata.
    ///
    /// `Ok(None)` represents a 404 deletion race. Message bodies are untrusted
    /// provider content and must be sanitized by the ingest/rendering boundary.
    pub async fn get_message(
        &self,
        access_token: &AccessToken,
        message_id: &str,
    ) -> Result<Option<MicrosoftGraphMessage>, EmailApiError> {
        let mut url = self.graph_url(&["me", "messages", message_id]);
        url.query_pairs_mut()
            .append_pair("$select", MESSAGE_SELECT)
            .append_pair(
                "$expand",
                "attachments($select=id,name,contentType,size,isInline,contentId)",
            );
        let response = self
            .send_get(url, access_token, Some("IdType=\"ImmutableId\""))
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = successful_response(response).await?;
        let message: MessageWire = decode_json_limited(response).await?;
        Ok(Some(message.try_into()?))
    }

    /// Downloads raw file or item attachment bytes with the configured size cap.
    pub async fn get_attachment_bytes(
        &self,
        access_token: &AccessToken,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<Vec<u8>, EmailApiError> {
        let url = self.graph_url(&[
            "me",
            "messages",
            message_id,
            "attachments",
            attachment_id,
            "$value",
        ]);
        let response = self
            .send_get(url, access_token, Some("IdType=\"ImmutableId\""))
            .await?;
        let response = successful_response(response).await?;
        read_body_limited(response, self.max_attachment_bytes.get(), "attachment").await
    }

    fn validate_folder_walk_state(
        &self,
        state: &MicrosoftGraphFolderWalkCursor,
    ) -> Result<(), EmailApiError> {
        let limit = self.max_folder_walk_folders.get();
        if state.discovered_folder_ids.len() > limit || state.pending_parents.len() > limit {
            return Err(invalid_response(
                "folder walk cursor exceeds the configured folder limit",
            ));
        }

        let discovered = state
            .discovered_folder_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        if discovered.len() != state.discovered_folder_ids.len()
            || (state.current_parent_id.is_none()
                && state.current_page.is_none()
                && !state.discovered_folder_ids.is_empty())
            || state
                .current_parent_id
                .as_deref()
                .is_some_and(|folder_id| !discovered.contains(folder_id))
            || state
                .pending_parents
                .iter()
                .any(|parent| !discovered.contains(parent.folder_id.as_str()))
        {
            return Err(invalid_response("folder walk cursor state is inconsistent"));
        }

        let pending = state
            .pending_parents
            .iter()
            .map(|parent| parent.folder_id.as_str())
            .collect::<HashSet<_>>();
        if pending.len() != state.pending_parents.len()
            || state
                .current_parent_id
                .as_deref()
                .is_some_and(|folder_id| pending.contains(folder_id))
        {
            return Err(invalid_response(
                "folder walk cursor contains duplicate pending parents",
            ));
        }
        Ok(())
    }

    fn folder_collection_url(&self, parent_folder_id: Option<&str>) -> Url {
        match parent_folder_id {
            Some(parent_id) => self.graph_url(&["me", "mailFolders", parent_id, "childFolders"]),
            None => self.graph_url(&["me", "mailFolders"]),
        }
    }

    fn graph_url(&self, segments: &[&str]) -> Url {
        let mut url = self.trusted_origin.clone();
        let mut path = url
            .path_segments_mut()
            .expect("validated HTTP(S) origins support path segments");
        path.clear();
        path.push(GRAPH_V1_SEGMENT);
        for segment in segments {
            path.push(segment);
        }
        drop(path);
        url
    }

    fn validated_continuation(
        &self,
        candidate: &str,
        expected_path: &str,
    ) -> Result<Url, EmailApiError> {
        let url = Url::parse(candidate)
            .map_err(|_| invalid_response("continuation link is not an absolute URL"))?;
        let same_origin = url.scheme() == self.trusted_origin.scheme()
            && url.host_str() == self.trusted_origin.host_str()
            && url.port_or_known_default() == self.trusted_origin.port_or_known_default();
        if !same_origin
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || url.path() != expected_path
            || !url.path().starts_with("/v1.0/me/mailFolders")
        {
            return Err(invalid_response(
                "continuation link escaped the trusted mailbox collection",
            ));
        }
        Ok(url)
    }

    async fn send_get(
        &self,
        url: Url,
        access_token: &AccessToken,
        prefer: Option<&str>,
    ) -> Result<Response, EmailApiError> {
        let mut request = self
            .client
            .get(url)
            .header(ACCEPT, "application/json")
            .bearer_auth(access_token.expose_secret());
        if let Some(prefer) = prefer {
            request = request.header(PREFER, prefer);
        }
        request.send().await.map_err(transport_error)
    }
}

impl Default for MicrosoftGraphMailClient {
    fn default() -> Self {
        Self::new().expect("the built-in Microsoft Graph client configuration is valid")
    }
}

impl MailboxAttachmentClient for MicrosoftGraphMailClient {
    async fn get_attachment(
        &self,
        access_token: &AccessToken,
        provider_message_id: &str,
        provider_attachment_id: &str,
    ) -> Result<Vec<u8>, EmailApiError> {
        self.get_attachment_bytes(access_token, provider_message_id, provider_attachment_id)
            .await
    }
}

mod models;
pub use models::*;

use models::{FolderCollectionWire, MessageDeltaWire, MessageWire, PendingFolderWalkParent};
fn validate_origin(origin: &Url) -> Result<(), MicrosoftGraphConfigError> {
    let loopback_http = origin.scheme() == "http"
        && origin
            .host_str()
            .and_then(|host| host.parse::<std::net::IpAddr>().ok())
            .is_some_and(|address| address.is_loopback());
    let valid_scheme = origin.scheme() == "https" || loopback_http;
    if !valid_scheme
        || origin.host_str().is_none()
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.query().is_some()
        || origin.fragment().is_some()
        || origin.path() != "/"
    {
        return Err(MicrosoftGraphConfigError::InvalidOrigin(
            "expected a bare HTTPS origin (HTTP is allowed only for loopback tests)".to_string(),
        ));
    }
    Ok(())
}

async fn successful_response(response: Response) -> Result<Response, EmailApiError> {
    if response.status().is_success() {
        return Ok(response);
    }

    let status = response.status();
    let retry_after = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_retry_after);
    match status.as_u16() {
        401 => Err(EmailApiError::AuthRequired),
        403 => Err(EmailApiError::Forbidden),
        404 => Err(EmailApiError::NotFound),
        410 => Err(EmailApiError::OutdatedCursor),
        429 => Err(EmailApiError::RateLimited {
            retry_after,
            origin: RateLimitOrigin::Provider,
        }),
        500..=599 => Err(EmailApiError::Transient {
            message: format!("Microsoft Graph returned HTTP {status}"),
        }),
        _ => Err(EmailApiError::Permanent {
            message: format!("Microsoft Graph returned HTTP {status}"),
        }),
    }
}

async fn decode_json_limited<T>(response: Response) -> Result<T, EmailApiError>
where
    T: DeserializeOwned,
{
    let bytes = read_body_limited(response, MAX_JSON_RESPONSE_BYTES, "JSON response").await?;
    serde_json::from_slice(&bytes).map_err(|error| EmailApiError::Permanent {
        message: format!("failed to decode Microsoft Graph response: {error}"),
    })
}

async fn read_body_limited(
    mut response: Response,
    limit: usize,
    kind: &str,
) -> Result<Vec<u8>, EmailApiError> {
    if response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > limit)
    {
        return Err(EmailApiError::Permanent {
            message: format!("Microsoft Graph {kind} exceeds the configured size limit"),
        });
    }

    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(EmailApiError::Permanent {
                message: format!("Microsoft Graph {kind} exceeds the configured size limit"),
            });
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn parse_retry_after(value: &str) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let when = httpdate::parse_http_date(value).ok()?;
    Some(
        when.duration_since(SystemTime::now())
            .unwrap_or(Duration::ZERO),
    )
}

fn transport_error(error: reqwest::Error) -> EmailApiError {
    EmailApiError::Transient {
        message: format!("Microsoft Graph transport error: {}", error.without_url()),
    }
}

fn invalid_response(message: &str) -> EmailApiError {
    EmailApiError::Permanent {
        message: format!("Microsoft Graph returned an invalid response: {message}"),
    }
}

#[cfg(test)]
mod test;
