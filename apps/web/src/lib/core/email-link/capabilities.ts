/**
 * Real backend value: `crates/models_email/src/email/api/link.rs` on this
 * branch defines `UserProvider::Microsoft`, serialized `UPPERCASE` as
 * `"MICROSOFT"` (see `UserProvider::as_str`). The frontend's generated
 * OpenAPI client (`service-email/generated/schemas/userProvider.ts`) has not
 * been regenerated from it yet — it still only lists `GMAIL` — so this
 * constant exists to reference the value by name until that regen happens.
 * Do not regenerate the client to "add" it; ask backend when its OpenAPI
 * spec is ready to pull. See docs/AGENT_GUIDE/email-microsoft365.md.
 */
export const MICROSOFT_PROVIDER = 'MICROSOFT' as const;

/** The only provider value this module treats as ever writable. */
export const GMAIL_PROVIDER = 'GMAIL' as const;

/**
 * `UserProvider` widened with the real (already-shipped server-side, not yet
 * regenerated client-side) `MICROSOFT` value, so the settings UI and
 * capability checks can be written against the shape the backend already
 * sends without waiting on a schema regen.
 */
export type ProposedEmailProvider = typeof GMAIL_PROVIDER | typeof MICROSOFT_PROVIDER;

/** True for a Microsoft link; false for Gmail or unknown/absent providers. */
export function isMicrosoftProvider(
  provider: ProposedEmailProvider | string | null | undefined
): boolean {
  return provider === MICROSOFT_PROVIDER;
}

/**
 * The subset of a `Link` this module reads. `is_read_only` is the real field
 * `crates/models_email/src/email/api/link.rs` already computes server-side
 * (`is_read_only = matches!(provider, UserProvider::Microsoft)`) and sends on
 * the wire today, ahead of the frontend's generated `Link` type declaring it
 * — so it's typed optional here and read defensively rather than assumed
 * absent. Once `service-email/generated/schemas/link.ts` is regenerated to
 * include it, this type can be dropped in favor of the real one.
 */
export interface LinkReadOnlyFields {
  provider: ProposedEmailProvider | string;
  is_read_only?: boolean;
}

/**
 * What the UI may let the user do with a linked mailbox. `canMarkRead` covers
 * both an explicit "mark as read" action and read state implied by opening a
 * message — Microsoft links deny both, since the backend adapter only holds a
 * read-only Microsoft Graph grant and any write back to the mailbox
 * (including a read receipt) is out of scope for it.
 */
export interface EmailCapabilities {
  readOnly: boolean;
  canSend: boolean;
  canReply: boolean;
  canDraft: boolean;
  canMarkRead: boolean;
  canLabel: boolean;
  canMove: boolean;
  canDelete: boolean;
  canShare: boolean;
}

const FULL_ACCESS: EmailCapabilities = {
  readOnly: false,
  canSend: true,
  canReply: true,
  canDraft: true,
  canMarkRead: true,
  canLabel: true,
  canMove: true,
  canDelete: true,
  canShare: true,
};

const READ_ONLY: EmailCapabilities = {
  readOnly: true,
  canSend: false,
  canReply: false,
  canDraft: false,
  canMarkRead: false,
  canLabel: false,
  canMove: false,
  canDelete: false,
  canShare: false,
};

/**
 * Whether a link is read-only. **Fails closed**: an unresolved link (not yet
 * loaded, or simply unknown — `undefined`/`null`) is read-only, not writable
 * — the inverse of this module's earlier behavior. Mutations are allowed only
 * for a link **known** to be Gmail and not explicitly marked read-only by the
 * backend:
 *
 * - Microsoft is unconditionally read-only. Its `is_read_only` field is never
 *   even consulted, so a bug or a stale/omitted value on the wire can never
 *   produce a false "writable" result for a Microsoft link — there is no
 *   client-visible path to override this.
 * - Gmail is writable only when the backend hasn't marked it `is_read_only`.
 * - Any other/unrecognized provider value fails closed (read-only), the same
 *   as an unresolved link — an unmodeled provider must never be silently
 *   treated as Gmail-equivalent.
 */
export function isLinkReadOnly(
  link: LinkReadOnlyFields | null | undefined
): boolean {
  if (!link) return true;
  if (isMicrosoftProvider(link.provider)) return true;
  if (link.provider !== GMAIL_PROVIDER) return true;
  return link.is_read_only === true;
}

/**
 * Frontend-side capability computation for a linked mailbox, used to
 * disable/label send, reply, draft, mark-read, label, move, delete, and
 * share controls (including keyboard, context-menu, and bulk multi-select
 * paths that share the same command functions — see
 * `thread-action-adapter.tsx` and `next-soup/actions/entity-capability.ts`)
 * before a request round-trips. This is NOT the security boundary —
 * email-service and auth-service enforce the real rule server-side (the
 * backend is the final authorizer for Microsoft mail actions), so a client
 * that ignores this file entirely must still be rejected there.
 */
export function getEmailCapabilities(
  link: LinkReadOnlyFields | null | undefined
): EmailCapabilities {
  return isLinkReadOnly(link) ? READ_ONLY : FULL_ACCESS;
}
