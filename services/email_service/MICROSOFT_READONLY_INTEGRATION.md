# Microsoft read-only mailbox integration

The Microsoft provider is additive. It never replaces an existing Gmail link.
One active Microsoft mailbox is allowed per FusionAuth user in the initial scope.
Every mailbox and folder query is scoped by both `email_links.id` and the owning
`fusionauth_user_id`; delegated or guessed link IDs cannot read or advance cursors.

## Folder mapping

Graph folder IDs are the durable identity. Display names are mutable metadata and
must never be used as keys. The optional `well_known_name` records semantic hints:

| Graph well-known folder | Station projection |
| --- | --- |
| `inbox` | Inbox visibility |
| `sentitems` | Sent/system label |
| `drafts` | Provider draft visibility only; Station cannot edit Graph drafts |
| `deleteditems` | Trash/deleted visibility |
| `junkemail` | Spam/junk visibility |
| other/custom folders | Read-only user label/folder projection |

Folder moves update `parent_folder_id` on the stable folder row. A folder absent
from traversal is marked `is_deleted`; local messages are reconciled by the sync
worker and are not deleted from Graph. Message tombstones and parent-folder moves
remain provider events, not a Gmail-style global history stream.

## Cursor contract

`email_microsoft_folder_sync` stores a cursor per `(link_id, folder_id)`. Cursor
commits use `cursor_generation` compare-and-swap. The worker must persist the
entire page batch idempotently before advancing the cursor. A stale worker gets no
row back and must reload. Graph expired-cursor recovery clears the cursor through
the generation-guarded invalidation function, then performs bounded backfill.

## Read-only capability

Microsoft links return `is_read_only: true`. Domain policy rejects draft writes,
sends, scheduled sends, mark-read/unread, archive, label mutation, and draft
deletion before a Gmail mutation queue or provider API can be reached. Disconnect
blocks only the selected owner's local link and folder jobs. No Microsoft POST,
PUT, PATCH, DELETE, send, mark-read, label, move, delete, or auto-share operation
is authorized by this integration.

## Required authentication-service contract

The separate auth owner must provide an internal client that:

1. Accepts only the authenticated owner identity at the internal auth-client boundary.
   The email service separately verifies the persisted active link and compares
   the returned verified tenant/object identity before using the token.
2. Returns a short-lived read-only `AccessToken` only for an active owner-scoped
   Microsoft link.
3. Distinguishes missing/revoked grants from transient token refresh failures.
4. Rotates encrypted refresh tokens without exposing them to email service logs.
5. Disables the owner-scoped local grant and cancels pending consent flows on
   disconnect before final link deletion. This is not remote Entra consent revocation.

The email service must not read `microsoft_oauth_grants` directly or infer mailbox
ownership from an existing Microsoft identity link.

## Native sync implementation contract

`POST /email/links/microsoft/init` accepts no mailbox identity from the caller. It resolves the
authenticated FusionAuth owner, reads the auth-service owner-scoped status, provisions the
verified `(tenant_id, object_id, normalized email)` as an additive Microsoft link, and enqueues
`MicrosoftSync::DiscoverFolders`. Gmail links are unaffected and remain the default provider.

The refresh handler buckets active Microsoft links on the configured health-poll interval. The existing link-manager SQS queue carries bounded `DiscoverFolders`, `SyncFolder`, and
`PurgeFolder` units. Folder traversal and message delta cursors are persisted after each durable
batch with generation CAS. Expired delta cursors are invalidated first, then the local folder
projection is cleared in batches of 500 before a fresh delta snapshot starts. Message state tracks
the current folder by immutable Graph message ID, so a tombstone from an old folder cannot delete
a message already projected in its new folder.

The worker projects into the native `email_threads`, `email_messages`, `email_contacts`, labels,
message-labels, and `email_attachments` tables. HTML goes through `email_utils::sanitize_email_html`.
Attachment metadata retains Graph IDs and the attachment read endpoint dispatches to Graph GET for
Microsoft links. The `microsoft_graph_readonly` Cargo feature remains excluded from defaults.

## Mailbox selection contract

Single-inbox routes accept `X-Email-Link-Id: <uuid>`. The extractor selects that link only when it
is in the authenticated caller's owned/delegated inbox set. Without the header it selects the
caller's own primary link, not a delegated mailbox's primary link. Because Gmail remains the
default/primary integration, a secondary Microsoft mailbox must normally be selected explicitly.
Mutation handlers then validate the persisted link that owns the target entity; a writable Gmail
selection cannot authorize mutation of a Microsoft-owned message, thread, draft, label, or
attachment.

## Attachment boundaries

Raw attachment GET dispatches by the attachment owner link: Gmail uses the existing email API and
Microsoft uses Graph GET. The legacy SFS/DSS document/media ingestion helper is Gmail-specific and
rejects Microsoft explicitly before any Gmail dispatch, including cached document-ID fast paths.
