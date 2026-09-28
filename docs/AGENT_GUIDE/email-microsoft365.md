# Microsoft 365 (Outlook) read-only mailbox — frontend slice

Status: **frontend UI only, feature-flagged off by default**. This document
is the contract reference for that slice — read it before touching any of
the files it lists. Verified against branch `station/m365-readonly` at
commit `41b6b179` ("Add Microsoft read-only mailbox persistence"); re-check
against current source before relying on any endpoint shape here.

## What this is

Per-user, delegated, **read-only** Microsoft 365 mailbox connection,
alongside (not replacing) Gmail. Explicitly **not** Microsoft
identity/login — it's a second mailbox grant a user opts into from
Settings, additive to the same generic `email_links` list Gmail inboxes
live in.

Gated by `enableMicrosoft365Email` (`apps/web/src/lib/core/constant/featureFlags.ts`).
No PostHog flag has been created for it — until one is, the flag resolves to
`false` everywhere. Override locally with `VITE_ENABLE_MICROSOFT365_EMAIL=true`.

## Real backend contract

### `POST /microsoft-mailbox/connect` (auth-service)

`services/authentication_service/src/api/microsoft_mailbox.rs` (`fn start`).
No request body or query params. Returns:

```json
{ "authorizationUrl": "https://login.microsoftonline.com/..." }
```
(camelCase — different from Gmail's snake_case `authorization_url`.) Bridge:
`authServiceClient.initMicrosoftMailboxConnect`.

503 when `ctx.microsoft_mailbox_oauth_enabled` is off server-side, in
addition to (not instead of) the frontend's own `enableMicrosoft365Email`
gate.

### `GET /microsoft-mailbox/callback` (auth-service) — not a frontend-callable API

Same file, `fn callback`. Microsoft's OAuth `redirect_uri`, hardcoded to
`{BASE_URL}/microsoft-mailbox/callback` — the browser navigates here as part
of the OAuth chain; the frontend never calls it directly. On success or
denial it **303-redirects** (not raw JSON) to a fixed, deployment-configured
completion URL with a result flag:

```
{microsoft_mailbox_completion_url}?microsoftMailbox=connected
{microsoft_mailbox_completion_url}?microsoftMailbox=denied
```

The frontend route for that is `/settings/connections`
(`MicrosoftMailboxCompletionRoute`, registered as a top-level route in
`routes/Root.tsx` exactly like Gmail's `CALLBACK_PATH`/`LINK_CALLBACK_PATH`).
**This depends on `microsoft_mailbox_completion_url` being configured to
point at that path on this deployment's origin** — that's backend/ops
config this slice cannot verify from source alone; confirm it's set before
enabling the feature flag anywhere real traffic reaches it.

There is no `original_url`/return-layout parameter on `/connect` the way
Gmail's `/link/gmail` has — the completion redirect target is fixed
regardless of where the user started the flow, so
`useAddMicrosoftMailboxFlow` has no return-layout stash and the user always
lands on Settings > Connected accounts after consent.

### `GET /microsoft-mailbox` and `DELETE /microsoft-mailbox` (auth-service)

Same file, `fn status` / `fn disconnect`. Status:

```json
{ "connected": true, "email": "user@contoso.com", "tenantId": "...", "objectId": "..." }
```
(all four fields `null`/absent when `connected: false`; no tokens ever
included.) Bridge: `authServiceClient.getMicrosoftMailboxStatus`. Not
currently called by this slice's UI — see "What calls what" below for why —
kept available for a future direct status check.

Disconnect revokes the grant. Bridge:
`authServiceClient.disconnectMicrosoftMailbox`. **Not** the normal disconnect
path in this UI: `useRemoveInboxMutation` (`emailClient.deleteLink`) already
triggers this same revoke server-side —
`services/email_service/src/pubsub/link_manager/process.rs`'s
`UserProvider::Microsoft` arm calls
`ctx.auth_service_client.disconnect_microsoft_mailbox(...)` before tearing
down the link. `MicrosoftEmailCard`'s "Remove inbox" already uses the
generic `useRemoveInboxMutation`, so no separate frontend call was added for
the normal case. `disconnectMicrosoftMailbox` exists only for the edge case
of a grant with no provisioned link (init never ran/failed) — not wired into
any UI in this slice; a future "cancel connection" affordance for that state
would use it.

### `POST /email/links/microsoft/init` (email-service)

`services/email_service/src/api/email/links/microsoft.rs` (`init_handler`).
**No request body.** Re-verifies the grant against auth-service's own status
server-side and provisions/reactivates the Microsoft `email_links` row from
that verified `(tenant_id, object_id, email)` — never from anything the
caller supplies. Returns:

```json
{ "linkId": "...", "email": "user@contoso.com", "created": true }
```

409 CONFLICT if auth-service reports no active grant. Bridge:
`emailClient.initMicrosoftLink`. Called from `MicrosoftMailboxCompletionRoute`
on `?microsoftMailbox=connected` — **this is the actual provisioning call**;
everything before it (including the query param) is only a hint to try it.

### `Link.provider` / `Link.is_read_only` (email-service)

`crates/models_email/src/email/api/link.rs`:

```rust
pub enum UserProvider { Gmail, Microsoft }   // wire: "GMAIL" | "MICROSOFT"
pub struct Link { ..., pub provider: UserProvider, pub is_read_only: bool, ... }
```

Not yet in the frontend's generated `service-email/generated/schemas/*`
(still only `GMAIL`, no `is_read_only`). This slice does not regenerate that
client. `core/email-link/capabilities.ts` names the real wire value
(`MICROSOFT_PROVIDER = 'MICROSOFT'`) and reads `is_read_only` defensively.
**Action needed**: regenerate the `service-email` OpenAPI client once its
spec publishes these fields, then delete `LinkReadOnlyFields` in favor of
the real generated type.

## What calls what (the actual flow, end to end)

1. `MicrosoftEmailCard` "Connect" → `useAddMicrosoftMailboxFlow` →
   `POST /microsoft-mailbox/connect` → full-page redirect to Microsoft.
2. Microsoft redirects to auth-service's own `/microsoft-mailbox/callback`
   (never touches the SPA).
3. auth-service 303-redirects to `/settings/connections?microsoftMailbox=...`.
4. `MicrosoftMailboxCompletionRoute` reads that flag. On `connected`, it
   calls `POST /email/links/microsoft/init` — which **independently
   re-verifies** the grant server-side before provisioning anything, so a
   forged/stale query param cannot provision a mailbox that isn't actually
   authorized (backend answers 409 instead). This satisfies "invoke init on
   verified completion, not just poll hoping a link appears" and "no caller
   email proves ownership" — nothing client-supplied is ever treated as
   ownership proof anywhere in this chain.
5. On success, `invalidateEmailLinks()` refetches the generic, provider-
   agnostic links list — the same list Gmail inboxes render from — so
   `MicrosoftEmailCard` and every other consumer of `useEmailLinksQuery`
   picks up the new mailbox without a page reload.
6. Disconnect: `MicrosoftEmailCard`'s existing "Remove inbox" →
   `useRemoveInboxMutation` → `DELETE /email/links/{id}` → backend cascade
   revokes the auth grant automatically (see above). No separate frontend
   call needed.

Nothing in this chain uses `GET /microsoft-mailbox` (status) or blind
polling — the earlier draft of this slice used a `sessionStorage` "connecting"
flag plus a timed poll of the links list because no completion redirect
existed yet; that workaround has been removed now that one does.

## Capabilities: fails closed, Gmail-only allowlist

`core/email-link/capabilities.ts`'s `isLinkReadOnly`/`getEmailCapabilities`:

- **Fails closed.** An unresolved link (not yet loaded, or simply unknown —
  `undefined`/`null`) is read-only, not writable. This is a real, deliberate
  behavior change from this slice's first draft, which failed *open*
  (unresolved → full access) — that was backwards for a security-adjacent
  UX guard.
- **Allows mutation only for a link known to be Gmail** and not explicitly
  marked `is_read_only` by the backend. Any other/unrecognized provider
  value fails closed, the same as an unresolved link.
- **Microsoft is unconditionally read-only.** Its `is_read_only` field is
  never even consulted for the Microsoft branch, so a bug or a stale/omitted
  value on the wire can never produce a false "writable" result for a
  Microsoft link — there's no client-visible path to override this.

Because "unresolved" now means "read-only" instead of "writable", every
gated read site (`thread-reply-input.tsx`, `thread-action-adapter.tsx`,
`EmailThreadLoadGate.tsx`) reads the links query through `queryReadyGate`
(`lib/queries/gate.ts`) rather than a raw `.data` access — a pending/paused
query's `.data` still suspends the nearest `<Suspense>` even past an
`isLoading` check (see `apps/web/AGENTS.md`), and reading it unguarded here
would have both re-introduced that class of bug *and* undermined the new
fail-closed intent by racing the Suspense boundary. Net effect: a Gmail
composer/action briefly renders its read-only fallback instead of a writable
UI for the short window before the links list first resolves, rather than a
flash of writable UI that isn't actually known-safe yet. That window is
small in practice — `useEmailLinksQuery` is fetched early (Settings,
`EmailPermissionsBanner`, Layout) — but it is a real, intentional trade-off,
not an oversight.

## What's gated as read-only, and where

All UX-only — **email-service is the final enforcer regardless of whether a
client goes through any of this.**

### Single-thread / open-thread view
- **Compose/reply/forward/draft/send** — `thread-reply-input.tsx` replaces
  the whole `ReplyInputView` composer with a static notice.
- **Archive / mark done / mark not done / mark read / mark unread / block
  sender / mark sender signal / mark sender noise** —
  `thread-action-adapter.tsx`, the single factory producing
  `EmailThreadCommands`; keyboard shortcuts (`core/thread-keyboard.ts`),
  context-menu items, and header/row buttons all call through the same
  function references.
- **Implicit mark-read on open** — `EmailThreadLoadGate.tsx` skips mounting
  `EmailDebouncedReadMarker` entirely for a read-only thread.
- **Attachment opening** — `attachment-action-adapter.ts` routes a
  confirmed-Microsoft attachment to the direct presigned-URL path instead of
  the Gmail-only document-conversion endpoint that was previously called
  unconditionally (a real bug, not just a capability gap — see "Attachments:
  provider-aware opener" above).

### Bulk multi-select / keyboard / context menu (cross-entity "soup" system)
New in this pass: `core/email-link/entity-capability.ts`'s
`isEmailEntityWritable(entity)` resolves a soup `EntityData` email row's
mailbox capability from the `EmailEntity.linkId` field (already present on
the entity type) against the **synchronously-read TanStack Query cache**
(`queryClient.getQueryData(emailKeys.links.queryKey)`), not a reactive hook —
these action factories run imperatively from bulk toolbars, keyboard
handlers, and context menus alike, with no single component-scoped place to
thread a reactive query through all of them. Also fails closed (cache
unloaded → not writable); an entity with no explicit `linkId` (the normal,
intentional state for a primary-inbox thread) resolves against the loaded
primary link instead of failing closed on a missing id, so ordinary Gmail
primary-inbox bulk actions are unaffected.

Wired into, in `features/next-soup/actions/`:
- `make-mark-done-action.ts` (archive/mark done — both `canExecute` and the
  `isMarkDoneTarget` filter used by `execute`/`executeWithSoup`)
- `make-mark-not-done-action.ts` (unarchive)
- `make-mark-unread-action.ts` (both `makeMarkUnreadAction` and
  `makeMarkReadAction` — the bulk equivalent of the implicit-read guard)
- `make-block-sender-action.ts`
- `make-sender-filter-action.ts` (shared by mark-signal **and** mark-noise —
  one file covers both)
- `make-delete-action.ts` — `canExecute`, the `executeWithSoup` trash lane
  (`emailEntities` filter), **and** the non-soup `execute` path's `rest`
  filter (closed in this pass — a caller invoking `execute` directly with a
  mixed selection can no longer hand a read-only email through to
  `openBulkEditModal`'s own delete mutation).

## Attachments: provider-aware opener

`GET /email/attachments/{id}/document_id` (`getEmailAttachmentDocument`, the
Gmail attachment-open path) **intentionally rejects Microsoft** with 400
`ProviderReadOnly` — see
`services/email_service/src/api/email/attachments/get_document_id.rs`,
`verify_access_and_get_owner`: "reject Microsoft before entering the
Gmail-backed SFS/DSS ingestion helper." That endpoint converts the
attachment into a Macro-native document (SFS/DSS upload) for in-app
viewing/editing — out of scope for a read-only grant. Before this fix,
`attachment-action-adapter.ts` called it unconditionally, so a real
Microsoft attachment click always failed.

The endpoint that already works for both providers is `GET
/email/attachments/{id}` (`emailClient.getAttachmentUrl`,
`services/email_service/src/api/email/attachments/get.rs`): it fetches the
attachment from whichever provider owns the link server-side (Gmail or
Microsoft Graph — **no token of either kind ever reaches the browser**),
uploads it to S3, and returns `{ attachment: { data_url, ... } }`, a
short-lived presigned CloudFront HTTPS URL. `createEmailAttachmentOpener`
now takes the open thread's link accessor, resolves its provider from the
same `useEmailLinksQuery` cache every other gate uses, and:
- **Gmail (or unresolved/loading link)**: unchanged existing behavior —
  `getEmailAttachmentDocument` → Macro document → opened in the split view.
  Deliberately does *not* fail closed to the Microsoft path on an
  unresolved link, unlike the mutation capability guards — the asymmetric
  cost of guessing wrong here is losing the existing, working Gmail flow
  transiently, not a safety issue.
- **Confirmed Microsoft**: `emailClient.getAttachmentUrl` → validate the
  returned `data_url` is `https:` (`isSafeAttachmentUrl`; rejects
  `javascript:`/`data:`/`blob:`/`http:`/empty) → `window.open(url, '_blank',
  'noopener,noreferrer')`. No document conversion, no Graph token, no bearer
  value of any kind ever constructed or logged client-side.

## What's explicitly NOT covered (real gaps, not oversights)

- **Labels**: the only label-mutation call site found in this codebase is
  `lib/core/component/AI/component/tool/UpdateThreadLabels.tsx`, a chat-
  message *renderer* for an AI agent tool call the backend already executed
  server-side (already backend-enforced) — no manual "add label" UI button
  was found to gate. If one exists under a name this search missed, it needs
  `getEmailCapabilities`/`isEmailEntityWritable` too.
- **Shared-mailbox conflicts**: no detection exists for Microsoft at all
  (unlike Gmail's `SHARED_INBOX_CONFLICT`/`forceShare`) —
  `provision_microsoft_mailbox` has no owner-conflict concept in the source
  read for this slice. Per this task's instruction, Gmail's
  `ShareInboxConflictDialog`/`forceShare` is not reused for Microsoft; if
  backend adds conflict detection, it needs its own resolution UI.
- **`MicrosoftMailboxCompletionRoute`'s dependency on
  `microsoft_mailbox_completion_url`** being deployment-configured to this
  origin's `/settings/connections` — cannot be verified from frontend source
  alone; confirm before enabling the feature flag anywhere real traffic
  reaches it. Related, and now resolved: this path is also the real
  "Connected" settings tab's own URL (`SETTINGS_TAB_SLUGS.Connected ===
  'connections'`) — a bare/bookmarked visit with no `?microsoftMailbox=`
  therefore legitimately reaches this route first (solid-router ranks its
  static path above `LAYOUT_ROUTE`'s `/*splits`, same as the pre-existing
  Gmail callbacks). The component redirects immediately in that case with no
  backend calls and no toast, to a categorically different top-level path
  (`${DEFAULT_ROUTE}/~/settings/...`, never `/settings/...`), so it cannot
  re-match itself — no infinite loop, proven by
  `MicrosoftMailboxCompletionRoute.test.tsx`. This is a redirect-based
  resolution (an extra client-side bounce for that one direct-visit case),
  not a passthrough render of the real settings panel — rendering the panel
  directly from this top-level route would require reproducing the
  split-router's own provider/context tree, out of scope here. Not
  independently runtime-verifiable without a dev server; recommend a manual
  smoke test (visit `/settings/connections` with and without
  `?microsoftMailbox=connected`) before enabling broadly.
- **`useEmailLinksQuery`'s cache staleness for the bulk/soup path**: since
  `entity-capability.ts` reads the query cache synchronously rather than
  awaiting a fresh fetch, a just-connected or just-revoked Microsoft
  mailbox's writability could be briefly stale in an already-open list view
  until the links query's own refetch (`refetchOnWindowFocus: 'always'`,
  5 min `staleTime`) catches up. Backend remains the real enforcer regardless.

## Files touched by this slice

See the task's final report for the exact file list; kept out of this doc to
avoid two sources of truth that can drift.
