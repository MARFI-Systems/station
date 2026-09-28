import { createRoot, createSignal } from 'solid-js';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { EmailThread } from './core/email-thread';
import { thread } from './tests/fixtures';

const mocks = vi.hoisted(() => ({
  toastFailure: vi.fn(),
  links: [] as Array<{ id: string; provider: string; is_read_only?: boolean }>,
  linksLoaded: true,
  archiveThread: vi.fn(() => true),
  markThreadNotDone: vi.fn(() => true),
  markThreadUnread: vi.fn(() => true),
  markThreadRead: vi.fn(() => true),
  blockSenderWithToast: vi.fn(),
  markSenderSignalWithToast: vi.fn(),
  markSenderNoiseWithToast: vi.fn(),
}));

vi.mock('@core/component/Toast/Toast', () => ({
  toast: { failure: mocks.toastFailure, success: vi.fn() },
}));
vi.mock('@core/context/user', () => ({ useEmail: () => () => 'me@example.com' }));
vi.mock('@queries/email/link', () => ({
  useEmailLinksQuery: () => ({
    isPending: !mocks.linksLoaded,
    data: mocks.linksLoaded ? { links: mocks.links } : undefined,
  }),
  useNonPrimaryEmailLinkIdHeader: () => (linkId: string | undefined) => linkId,
}));
vi.mock('@queries/email/thread', () => ({
  blockSenderWithToast: mocks.blockSenderWithToast,
  markSenderSignalWithToast: mocks.markSenderSignalWithToast,
  markSenderNoiseWithToast: mocks.markSenderNoiseWithToast,
}));
vi.mock('./thread-completion-adapter', () => ({
  createThreadCompletionAdapter: () => ({
    archiveThread: mocks.archiveThread,
    isThreadDone: () => false,
    canMarkThreadNotDone: () => true,
    markThreadNotDone: mocks.markThreadNotDone,
  }),
}));
vi.mock('./thread-read-adapter', () => ({
  createThreadReadAdapter: () => ({
    markThreadUnread: mocks.markThreadUnread,
    markThreadRead: mocks.markThreadRead,
    isThreadMarkedUnread: () => false,
  }),
}));
vi.mock('./core/thread-messages', () => ({
  selectThreadSender: () => 'sender@example.com',
}));

import { createThreadActionAdapter } from './thread-action-adapter';

afterEach(() => {
  mocks.links = [];
  mocks.linksLoaded = true;
  vi.clearAllMocks();
});

function run(threadValue: EmailThread) {
  const [threadSource] = createSignal<EmailThread | undefined>(threadValue);
  let commands!: ReturnType<typeof createThreadActionAdapter>;
  const dispose = createRoot((d) => {
    commands = createThreadActionAdapter(() => threadValue.db_id, threadSource);
    return d;
  });
  return { commands, dispose };
}

describe('createThreadActionAdapter read-only guard', () => {
  it('passes every mutating command through for a Gmail-linked thread (unchanged default behavior)', () => {
    mocks.links = [{ id: 'inbox', provider: 'GMAIL', is_read_only: false }];
    const { commands, dispose } = run(thread([], { link_id: 'inbox' }));

    expect(commands.archiveThread()).toBe(true);
    expect(commands.markThreadNotDone()).toBe(true);
    expect(commands.markThreadUnread()).toBe(true);
    expect(commands.markThreadRead()).toBe(true);
    expect(commands.blockSender()).toBe(true);
    expect(commands.markSenderSignal()).toBe(true);
    expect(commands.markSenderNoise()).toBe(true);
    expect(mocks.toastFailure).not.toHaveBeenCalled();

    dispose();
  });

  it('blocks every mutating command for a read-only Microsoft-linked thread — the single choke point keyboard, context menu, and buttons all share', () => {
    mocks.links = [{ id: 'inbox', provider: 'MICROSOFT', is_read_only: true }];
    const { commands, dispose } = run(thread([], { link_id: 'inbox' }));

    expect(commands.archiveThread()).toBe(false);
    expect(commands.markThreadNotDone()).toBe(false);
    expect(commands.markThreadUnread()).toBe(false);
    expect(commands.markThreadRead()).toBe(false);
    expect(commands.blockSender()).toBe(false);
    expect(commands.markSenderSignal()).toBe(false);
    expect(commands.markSenderNoise()).toBe(false);

    expect(mocks.archiveThread).not.toHaveBeenCalled();
    expect(mocks.markThreadNotDone).not.toHaveBeenCalled();
    expect(mocks.markThreadUnread).not.toHaveBeenCalled();
    expect(mocks.markThreadRead).not.toHaveBeenCalled();
    expect(mocks.blockSenderWithToast).not.toHaveBeenCalled();
    expect(mocks.markSenderSignalWithToast).not.toHaveBeenCalled();
    expect(mocks.markSenderNoiseWithToast).not.toHaveBeenCalled();
    expect(mocks.toastFailure).toHaveBeenCalledTimes(7);

    dispose();
  });

  it('fails closed (blocks every mutating command) while the links list is still loading/unknown', () => {
    mocks.linksLoaded = false;
    const { commands, dispose } = run(thread([], { link_id: 'inbox' }));

    expect(commands.archiveThread()).toBe(false);
    expect(commands.markThreadRead()).toBe(false);
    expect(mocks.archiveThread).not.toHaveBeenCalled();
    expect(mocks.markThreadRead).not.toHaveBeenCalled();

    dispose();
  });

  it('leaves read-only status accessors untouched (they report state, not act)', () => {
    mocks.links = [{ id: 'inbox', provider: 'MICROSOFT', is_read_only: true }];
    const { commands, dispose } = run(thread([], { link_id: 'inbox' }));

    expect(commands.isThreadDone()).toBe(false);
    expect(commands.canMarkThreadNotDone()).toBe(true);
    expect(commands.isThreadMarkedUnread()).toBe(false);

    dispose();
  });
});
