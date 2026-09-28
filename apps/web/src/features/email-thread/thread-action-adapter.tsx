import { toast } from '@core/component/Toast/Toast';
import { useEmail } from '@core/context/user';
import { getEmailCapabilities } from '@core/email-link/capabilities';
import { useEmailLinksQuery, useNonPrimaryEmailLinkIdHeader } from '@queries/email/link';
import { queryReadyGate } from '@queries/gate';
import {
  blockSenderWithToast,
  markSenderNoiseWithToast,
  markSenderSignalWithToast,
} from '@queries/email/thread';
import { createMemo, type Accessor } from 'solid-js';
import type { EmailThreadCommands } from './context/email-thread-context';
import type { EmailThread } from './core/email-thread';
import { selectThreadSender } from './core/thread-messages';
import { createThreadCompletionAdapter } from './thread-completion-adapter';
import { createThreadReadAdapter } from './thread-read-adapter';

const READ_ONLY_MAILBOX_MESSAGE = "This mailbox is read-only — Macro can't change it.";

/**
 * Wraps a mutating command so every path that can invoke it — button click,
 * keyboard shortcut (`core/thread-keyboard.ts`), and context menu, which all
 * resolve to this same function reference through `EmailThreadCommands` — is
 * blocked identically for a read-only (currently: Microsoft) link. This is a
 * frontend UX guard only; email-service is the final enforcer regardless of
 * whether a caller reaches it through this wrapper.
 */
function guardMutation<Args extends unknown[]>(
  isReadOnly: Accessor<boolean>,
  action: (...args: Args) => boolean
): (...args: Args) => boolean {
  return (...args: Args) => {
    if (isReadOnly()) {
      toast.failure(READ_ONLY_MAILBOX_MESSAGE);
      return false;
    }
    return action(...args);
  };
}

export function createThreadActionAdapter(
  threadId: Accessor<string>,
  threadSource: Accessor<EmailThread | undefined>
): EmailThreadCommands {
  const toHeaderLinkId = useNonPrimaryEmailLinkIdHeader();
  const completion = createThreadCompletionAdapter(
    threadSource,
    toHeaderLinkId
  );
  const read = createThreadReadAdapter(threadId, threadSource, toHeaderLinkId);
  const currentUserEmail = useEmail();

  // Frontend-side UX guess only (see getEmailCapabilities) — mirrors the same
  // read-only check the composer uses (thread-reply-input.tsx), keyed off the
  // thread's own link rather than the active/primary one so a shared or
  // secondary Microsoft inbox is guarded too. Fails closed while the links
  // list is pending/unknown (queryReadyGate, not a raw `.data` read).
  const emailLinksQuery = useEmailLinksQuery();
  const isReadOnly = createMemo(() => {
    const linkId = threadSource()?.link_id;
    const link = queryReadyGate(emailLinksQuery)
      ? emailLinksQuery.data.links.find((l) => l.id === linkId)
      : undefined;
    return getEmailCapabilities(link).readOnly;
  });

  const getSenderEmail = (): string | undefined => {
    const thread = threadSource();
    return thread ? selectThreadSender(thread, currentUserEmail()) : undefined;
  };

  const blockSender = () => {
    const senderEmail = getSenderEmail();
    if (!senderEmail) return false;
    blockSenderWithToast(senderEmail, toHeaderLinkId(threadSource()?.link_id));
    return true;
  };

  const markSenderSignal = () => {
    const senderEmail = getSenderEmail();
    if (!senderEmail) return false;
    markSenderSignalWithToast(
      senderEmail,
      toHeaderLinkId(threadSource()?.link_id)
    );
    return true;
  };

  const markSenderNoise = () => {
    const senderEmail = getSenderEmail();
    if (!senderEmail) return false;
    markSenderNoiseWithToast(
      senderEmail,
      toHeaderLinkId(threadSource()?.link_id)
    );
    return true;
  };

  return {
    ...completion,
    archiveThread: guardMutation(isReadOnly, completion.archiveThread),
    markThreadNotDone: guardMutation(isReadOnly, completion.markThreadNotDone),
    ...read,
    markThreadUnread: guardMutation(isReadOnly, read.markThreadUnread),
    markThreadRead: guardMutation(isReadOnly, read.markThreadRead),
    blockSender: guardMutation(isReadOnly, blockSender),
    markSenderSignal: guardMutation(isReadOnly, markSenderSignal),
    markSenderNoise: guardMutation(isReadOnly, markSenderNoise),
  };
}
