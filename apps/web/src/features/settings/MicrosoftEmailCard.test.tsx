/**
 * @vitest-environment jsdom
 */

import type { Link, ListLinksResponse } from '@service-email/generated/schemas';
import { fireEvent, render } from '@solidjs/testing-library';
import type { UseQueryResult } from '@tanstack/solid-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MicrosoftEmailCard } from './MicrosoftEmailCard';

const mocks = vi.hoisted(() => ({
  links: [] as Link[],
  startAddMicrosoftMailbox: vi.fn(),
  removeMutate: vi.fn(),
  toastSuccess: vi.fn(),
  toastFailure: vi.fn(),
}));

vi.mock('@core/component/Toast/Toast', () => ({
  toast: { success: mocks.toastSuccess, failure: mocks.toastFailure },
}));
vi.mock('@core/context/user', () => ({ useUserId: () => () => 'macro|self' }));
vi.mock('@core/email-link', () => ({
  useEmailLinks: () => ({
    query: {
      data: { links: mocks.links },
    } as Partial<UseQueryResult<ListLinksResponse, Error>>,
  }),
}));
vi.mock('@core/email-link/microsoft-mailbox-flow', () => ({
  useAddMicrosoftMailboxFlow: () => mocks.startAddMicrosoftMailbox,
}));
vi.mock('@queries/email/link', () => ({
  useRemoveInboxMutation: () => ({ mutate: mocks.removeMutate }),
}));

function link(overrides: Partial<Link> = {}): Link {
  return {
    id: 'ms-1',
    macro_id: 'macro|self',
    is_primary: true,
    email_address: 'person@contoso.com',
    calendar_disabled: false,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    fusionauth_user_id: 'macro|self',
    has_calendar_data: false,
    is_sync_active: true,
    needs_calendar_permission: false,
    needs_reauth: false,
    provider: 'MICROSOFT' as Link['provider'],
    sync_status: 'UP_TO_DATE',
    settings: {},
    ...overrides,
  };
}

afterEach(() => {
  mocks.links = [];
  vi.clearAllMocks();
});

describe('MicrosoftEmailCard', () => {
  it('shows a Connect action and starts the Microsoft 365 flow when nothing is linked', async () => {
    mocks.links = [];
    const view = render(() => <MicrosoftEmailCard />);
    try {
      const connect = await view.findByText('Connect');
      fireEvent.click(connect);
      expect(mocks.startAddMicrosoftMailbox).toHaveBeenCalledTimes(1);
    } finally {
      view.unmount();
    }
  });

  it('ignores Gmail links and only lists Microsoft inboxes as read-only', async () => {
    mocks.links = [
      link({ id: 'gmail-1', provider: 'GMAIL' as Link['provider'] }),
      link({ id: 'ms-1', email_address: 'person@contoso.com' }),
    ];
    const view = render(() => <MicrosoftEmailCard />);
    try {
      expect(await view.findByText('person@contoso.com')).toBeTruthy();
      expect(view.queryByText('gmail-1')).toBeNull();
      expect(await view.findByText('Read-only')).toBeTruthy();
    } finally {
      view.unmount();
    }
  });

  it('confirms before removing a connected Microsoft inbox', async () => {
    mocks.links = [link({ id: 'ms-1', email_address: 'person@contoso.com' })];
    const view = render(() => <MicrosoftEmailCard />);
    try {
      fireEvent.click(await view.findByLabelText('Remove person@contoso.com'));
      fireEvent.click(await view.findByText('Remove'));
      expect(mocks.removeMutate).toHaveBeenCalledWith('ms-1');
    } finally {
      view.unmount();
    }
  });
});
