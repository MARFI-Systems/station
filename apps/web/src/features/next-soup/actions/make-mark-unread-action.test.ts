import type { EntityData } from '@entity';
import { afterEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  isEmailEntityWritable: vi.fn(() => true),
  markUnreadMutateAsync: vi.fn(async () => {}),
  markSeenMutateAsync: vi.fn(async () => {}),
  toastFailure: vi.fn(),
  toastSuccess: vi.fn(),
}));

// Capability resolution has its own dedicated tests
// (core/email-link/entity-capability.test.ts); default to writable so every
// existing case below keeps testing this action's own logic.
vi.mock('@core/email-link/entity-capability', () => ({
  isEmailEntityWritable: mocks.isEmailEntityWritable,
}));
vi.mock('@core/component/Toast/Toast', () => ({
  toast: { failure: mocks.toastFailure, success: mocks.toastSuccess },
}));
vi.mock('@queries/email/link', () => ({
  useNonPrimaryEmailLinkIdHeader: () => (linkId: string | undefined) => linkId,
}));
vi.mock('@queries/email/thread', () => ({
  useMarkThreadAsSeenMutation: () => ({
    mutateAsync: mocks.markSeenMutateAsync,
  }),
  useMarkThreadAsUnreadMutation: () => ({
    mutateAsync: mocks.markUnreadMutateAsync,
  }),
}));
vi.mock('@queries/soup/cache', () => ({ refetchSoupEntity: vi.fn() }));

import { makeMarkReadAction, makeMarkUnreadAction } from './make-mark-unread-action';

const readEmail = (id: string) =>
  ({ type: 'email', id, isRead: true }) as EntityData;
const unreadEmail = (id: string) =>
  ({ type: 'email', id, isRead: false }) as EntityData;

afterEach(() => {
  vi.clearAllMocks();
  mocks.isEmailEntityWritable.mockReturnValue(true);
});

describe('makeMarkUnreadAction', () => {
  it('marks a writable, currently-read email as unread', async () => {
    const action = makeMarkUnreadAction();
    await action.execute([readEmail('a')]);
    expect(mocks.markUnreadMutateAsync).toHaveBeenCalledWith(
      expect.objectContaining({ threadId: 'a' })
    );
  });

  it('never marks a read-only email as unread, the same choke point keyboard, context menu, and bulk multi-select all share', async () => {
    mocks.isEmailEntityWritable.mockReturnValue(false);
    const action = makeMarkUnreadAction();

    expect(action.canExecute(readEmail('ms-thread'))).toBe(false);
    await action.execute([readEmail('ms-thread')]);
    expect(mocks.markUnreadMutateAsync).not.toHaveBeenCalled();
  });
});

describe('makeMarkReadAction', () => {
  it('marks a writable, currently-unread email as read', async () => {
    const action = makeMarkReadAction();
    await action.execute([unreadEmail('a')]);
    expect(mocks.markSeenMutateAsync).toHaveBeenCalledWith(
      expect.objectContaining({ threadId: 'a' })
    );
  });

  it('never marks a read-only email as read (bulk equivalent of the implicit-read guard)', async () => {
    mocks.isEmailEntityWritable.mockReturnValue(false);
    const action = makeMarkReadAction();

    expect(action.canExecute(unreadEmail('ms-thread'))).toBe(false);
    await action.execute([unreadEmail('ms-thread')]);
    expect(mocks.markSeenMutateAsync).not.toHaveBeenCalled();
  });
});
