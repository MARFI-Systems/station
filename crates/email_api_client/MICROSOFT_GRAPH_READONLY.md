# Microsoft Graph read-only mail adapter

This crate exposes a bounded first core behind the `outbound-microsoft-graph` feature. It is a provider client, not a completed native Outlook inbox.

## Public contract

- `MicrosoftGraphMailClient::list_mail_folders`: lists root folders or one folder's direct children, includes hidden folders, and returns a resumable bounded page cursor.
- `MicrosoftGraphMailClient::walk_mail_folders`: advances a bounded, resumable recursive walk of the delegated user's `/me` folder tree and preserves display-name path components.
- `MicrosoftGraphMailClient::list_folder_message_delta`: reads a bounded number of pages for one folder and returns either a continuation cursor or a completed delta cursor.
- `MicrosoftGraphMailClient::resume_folder_message_delta`: resumes a stored cursor and, on one 410 expiry, restarts once with a typed `RestartedAfterExpiredCursor` result that requires caller-side folder rebuild mode through the replacement round's terminal delta cursor.
- `MicrosoftGraphMailClient::get_message`: gets one typed Graph message plus attachment metadata; a 404 deletion race is `Ok(None)`.
- `MicrosoftGraphMailClient::get_attachment_bytes`: downloads raw file or item attachment bytes with a hard size cap.
- `MicrosoftGraphMailClient` implements only the genuinely provider-neutral `MailboxAttachmentClient` port. It deliberately does not implement Gmail-shaped thread counts, global history, label mutation, subscriptions, sending, contacts, or blocklist ports.

Graph delta cursors are full opaque URLs because Graph embeds query shape and state in them. Before a bearer token is attached, every continuation and delta URL is checked against the configured trusted origin and the exact `/v1.0/me/mailFolders/...` path for the current operation. Message and attachment IDs are encoded as URL path segments. Delta tombstones are preserved, including Graph's `deleted` tombstone used for both deletion and a move out of the tracked folder.

## Required integration seams outside this crate

1. A Microsoft token source must read and refresh the approved grant without logging tokens. This adapter accepts the existing redacted `AccessToken` only.
2. Provider dispatch, Microsoft provider enums, database migrations, and account isolation must be added by the owning email/auth crates.
3. The sync owner must persist the recursive folder-walk cursor and one delta cursor per folder, resume continuation cursors before advancing a round, treat `RestartedAfterExpiredCursor` as a folder projection rebuild, and reconcile a move as the old-folder tombstone plus destination-folder upsert. Each user's durable state must remain isolated outside this crate.
4. The ingest boundary must sanitize Graph HTML bodies and map Graph-specific messages/categories/conversation IDs without inventing Gmail labels, thread counts, or history semantics.
5. Retry orchestration must honor `EmailApiError::RateLimited { retry_after, origin: Provider }`; `AuthRequired`, `Forbidden`, `NotFound`, and `OutdatedCursor` require distinct caller behavior.
6. Attachment persistence/scanning remains outside this client. The caller must choose a configured size limit and handle reference attachments, whose `$value` endpoint is not supported by Graph.
7. Tenant consent and scopes remain an external approval gate. Attachment bytes require `Mail.Read`; this crate performs no OAuth or real mailbox calls.
8. Native pipeline wiring, UI/provider picker, background jobs, subscriptions, and any write/send capability are explicitly not included.

The folder walk and one-shot 410 recovery were selectively adapted from OpenArchiver's pinned AGPL-3.0 Graph connector design. Provenance and modifications are recorded in `THIRD-PARTY_NOTICES.md`. No Inbox Zero application code was used.

## Synthetic tests written

All tests are in `src/outbound/microsoft_graph/test.rs` and use wiremock data only:

- `lists_folders_in_bounded_resumable_pages`
- `recursively_walks_delegated_me_folders_with_resumable_state`
- `bounds_recursive_folder_walk_state`
- `paginates_folder_delta_and_preserves_move_delete_tombstones`
- `restarts_once_when_a_per_folder_delta_cursor_expires`
- `rejects_cross_origin_and_cross_mailbox_continuations_before_request`
- `rejects_untrusted_next_link_before_attaching_bearer_again`
- `maps_retry_after_and_expired_delta_cursor`
- `gets_typed_message_and_bounds_attachment_bytes`
- `returns_none_for_message_deletion_race`
- `read_only_public_contract_and_path_encoding_compile_sanity`

## Checks actually run in this worktree

- `rustfmt --check` on all changed Rust files: passed.
- `cargo metadata --manifest-path crates/email_api_client/Cargo.toml --no-deps --format-version 1 --features outbound-microsoft-graph`: passed.
- `git diff --check`: passed.
- Static source audit confirmed the adapter contains no POST, PUT, PATCH, or DELETE request construction and no tracing/debug output of access tokens.

No Cargo build, check, clippy, or test command was run on the runtime VM, by design. CI or a permitted build host must compile and execute the tests before merge.
