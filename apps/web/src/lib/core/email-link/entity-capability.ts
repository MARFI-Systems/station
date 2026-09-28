import { queryClient } from '@queries/client';
import { emailKeys } from '@queries/email/keys';
import type { ListLinksResponse } from '@service-email/generated/schemas';
import { getEmailCapabilities, type LinkReadOnlyFields } from './capabilities';

/**
 * The minimal shape this module needs off a soup `EntityData` email row.
 * Kept structural (not imported from `@entity`) so this file has no
 * dependency on the entity package's full type surface.
 */
export interface EmailEntityLinkRef {
  type: string;
  linkId?: string;
}

/**
 * Synchronous, non-reactive resolution of an email soup entity's mailbox
 * capability from the already-fetched, provider-agnostic links list cache.
 *
 * Why non-reactive: the generic cross-entity "soup" action factories
 * (`next-soup/actions/*`) run imperatively — from a keyboard shortcut, a
 * context-menu click, or a bulk multi-select toolbar button — and are the
 * single choke point all three paths share for archive / mark done / mark
 * read / mark unread / block sender / mark signal / mark noise / delete.
 * There is no one component-scoped place to thread a reactive query through
 * all of them, so this reads the TanStack Query cache directly instead, the
 * same way `getSoupEntityById` and friends already do elsewhere in this
 * codebase (see `thread-completion-adapter.tsx`).
 *
 * Fails CLOSED: if the links list hasn't loaded yet (cache empty/undefined —
 * "unknown/loading"), every email entity is treated as not writable, not the
 * other way around. Once loaded, an entity with no explicit `linkId` (the
 * normal, intentional state for a primary-inbox thread — see
 * `useNonPrimaryEmailLinkIdHeader`) resolves against the loaded primary
 * link instead of failing closed on a missing id, so ordinary Gmail primary-
 * inbox bulk actions are unaffected. A link that cannot be matched by id at
 * all (stale/removed) also fails closed.
 */
export function isEmailEntityWritable(entity: EmailEntityLinkRef): boolean {
  if (entity.type !== 'email') return true;

  const cached = queryClient.getQueryData<ListLinksResponse>(
    emailKeys.links.queryKey
  );
  if (!cached) return false; // unknown/loading — fail closed

  const link: LinkReadOnlyFields | undefined = entity.linkId
    ? cached.links.find((l) => l.id === entity.linkId)
    : cached.links.find((l) => l.is_primary);

  return !getEmailCapabilities(link).readOnly;
}
