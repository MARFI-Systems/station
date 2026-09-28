/**
 * @vitest-environment jsdom
 */

import type { Link, ListLinksResponse } from '@service-email/generated/schemas';
import { fireEvent, render } from '@solidjs/testing-library';
import type { UseQueryResult } from '@tanstack/solid-query';
import type { JSX, ParentProps } from 'solid-js';
import { Show } from 'solid-js';
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
// The real `@ui` `Dialog` is Kobalte's modal dialog (focus trap, portal,
// animation timing) via the app's `Dialog` wrapper (components/ui/components/
// Dialog.tsx) — it doesn't reliably mount its portaled content in jsdom, so
// its "Remove" confirm button never appears for `findByText` to locate
// (see src/lib/core/comments/Thread.test.tsx for the same established
// pattern: mock `@ui` with plain DOM stand-ins rather than exercise the real
// modal machinery). These stand-ins preserve the actual open/close
// semantics (`Show when={props.open}`) and the compound `.Title`/
// `.Description`/`.Header`/`.Body` API `MicrosoftEmailCard` calls, so the
// test still exercises real conditional-render and prop-wiring logic — only
// the dialog/panel chrome itself is stubbed.
// `primitives.tsx` (`SettingsCard`/`IntegrationRow`/`SettingsRow`) and
// `integration-ui.tsx` (`ConnectAction`/`StatusDot`) also import `cn`/`Layer`
// from `@ui` — mocking the module replaces it for every importer in this
// test's tree, not just `MicrosoftEmailCard.tsx`'s own direct imports, so
// both need real-enough stand-ins too (same simplification
// `Thread.test.tsx` uses).
vi.mock('@ui', () => {
  const cn = (...values: unknown[]) => values.filter(Boolean).join(' ');
  const Layer = (props: ParentProps) => props.children;
  const Button = (
    props: ParentProps<{
      onClick?: (e: MouseEvent) => void;
      disabled?: boolean;
      'aria-label'?: string;
    }>
  ) => (
    <button
      type="button"
      disabled={props.disabled}
      aria-label={props['aria-label']}
      onClick={(e) => props.onClick?.(e)}
    >
      {props.children}
    </button>
  );
  const DialogRoot = (
    props: ParentProps<{ open: boolean; onOpenChange?: (open: boolean) => void }>
  ) => <Show when={props.open}>{props.children}</Show>;
  const Dialog = Object.assign(DialogRoot, {
    Title: (props: ParentProps) => <h2>{props.children}</h2>,
    Description: (props: ParentProps) => <p>{props.children}</p>,
  });
  const PanelRoot = (props: ParentProps) => <div>{props.children}</div>;
  const Panel = Object.assign(PanelRoot, {
    Header: (props: ParentProps) => <div>{props.children}</div>,
    Body: (props: ParentProps) => <div>{props.children}</div>,
  });
  const Tooltip = (props: ParentProps<{ label?: JSX.Element }>) => (
    <>{props.children}</>
  );
  return { Button, cn, Dialog, Layer, Panel, Tooltip };
});

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
