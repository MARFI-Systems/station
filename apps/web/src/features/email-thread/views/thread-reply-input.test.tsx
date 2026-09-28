import type { Link, ListLinksResponse } from '@service-email/generated/schemas';
import { render } from '@solidjs/testing-library';
import { QueryClient, QueryClientProvider } from '@tanstack/solid-query';
import { ok } from 'neverthrow';
import { createSignal, type JSX, onCleanup } from 'solid-js';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { EmailReplySession } from '../../email-compose/context/email-form-inputs';
import { createComposeContext } from '../../email-compose/tests/capabilities';
import type { EmailMessage } from '../../email-message/core/email-message';
import { EmailThreadStateProvider } from '../context/email-thread-state-context';
import { EmailThreadViewProvider } from '../context/email-thread-view-context';
import {
  createEmailThreadState,
  type EmailThreadState,
} from '../primitives/email-thread-state';
import { createThreadContext, message, thread } from '../tests/fixtures';
import { ThreadReplyInput } from './thread-reply-input';

const lifecycle = vi.hoisted(() => ({
  mounted: [] as string[],
  disposed: [] as string[],
}));
beforeEach(() => {
  lifecycle.mounted.length = 0;
  lifecycle.disposed.length = 0;
});
afterEach(() => {
  vi.clearAllMocks();
});

// Probe the wrapper's editor lifetime and focus handoff. This does not verify
// the real rich editor, which still depends on shared application providers.
vi.mock('../../email-compose/views/reply-input', () => ({
  ReplyInputView: (props: {
    replyingTo: () => { db_id: string };
    onEngaged: () => void;
    session: EmailReplySession;
  }) => {
    const id = props.replyingTo().db_id;
    lifecycle.mounted.push(id);
    onCleanup(() => lifecycle.disposed.push(id));
    return (
      <>
        <button onClick={props.onEngaged}>{id}</button>
        <button onClick={() => props.session.exitToThread('last')}>
          Exit reply
        </button>
      </>
    );
  },
}));

// The composer's read-only gate reads `useEmailLinksQuery`, which needs a
// QueryClientProvider ancestor; mock the client fetch it's built on rather
// than let it hit the network.
const linkFixtures = vi.hoisted(() => ({
  links: [] as Link[],
  getLinksOverride: undefined as (() => Promise<{ links: Link[] }>) | undefined,
}));
vi.mock('@service-email/client', () => ({
  emailClient: {
    getLinks: async (): Promise<ReturnType<typeof ok<ListLinksResponse>>> => {
      const body = linkFixtures.getLinksOverride
        ? await linkFixtures.getLinksOverride()
        : { links: linkFixtures.links };
      return ok(body);
    },
  },
}));

function inboxLink(overrides: Partial<Link> = {}): Link {
  return {
    id: 'inbox',
    macro_id: 'macro|self',
    is_primary: true,
    email_address: 'inbox@example.com',
    calendar_disabled: false,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    fusionauth_user_id: 'macro|self',
    has_calendar_data: false,
    is_sync_active: true,
    needs_calendar_permission: false,
    needs_reauth: false,
    provider: 'GMAIL',
    sync_status: 'UP_TO_DATE',
    settings: {},
    ...overrides,
  };
}

function ThreadTestProvider(props: {
  messages: EmailMessage[];
  children: (state: EmailThreadState) => JSX.Element;
}) {
  const context = createThreadContext({ thread: () => thread(props.messages) });
  const state = createEmailThreadState(context);
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return (
    <QueryClientProvider client={queryClient}>
      <EmailThreadViewProvider
        value={{
          thread: context,
          compose: createComposeContext(),
          rendering: {},
        }}
      >
        <EmailThreadStateProvider value={state}>
          {props.children(state)}
        </EmailThreadStateProvider>
      </EmailThreadViewProvider>
    </QueryClientProvider>
  );
}

it('preserves an engaged composer through a same-message update but resets it for a different reply target', () => {
  linkFixtures.links = [inboxLink()];
  const first = message('first');
  const second = message('second');
  const [target, setTarget] = createSignal(first);
  const view = render(() => (
    <ThreadTestProvider messages={[first, second]}>
      {() => <ThreadReplyInput replyingTo={target} />}
    </ThreadTestProvider>
  ));
  try {
    view.getByText('first').click();
    setTarget({ ...first, updated_at: '2026-09-05T00:00:00Z' });
    expect(lifecycle.mounted).toEqual(['first']);
    setTarget(second);
    expect(lifecycle.mounted).toEqual(['first', 'second']);
    expect(lifecycle.disposed).toEqual(['first']);
  } finally {
    view.unmount();
  }
});

it('returns focus to the owning thread when split panes contain the same message', () => {
  linkFixtures.links = [inboxLink()];
  vi.stubGlobal('CSS', { escape: (value: string) => value });
  const parent = message('shared-message');
  const Pane = () => (
    <ThreadTestProvider messages={[parent]}>
      {(state) => (
        <div ref={state.registerMessagesContainer}>
          <div tabIndex={0} data-testid="card">
            <div data-message-body-id={parent.db_id} />
          </div>
          <ThreadReplyInput replyingTo={() => parent} />
        </div>
      )}
    </ThreadTestProvider>
  );
  const view = render(() => (
    <>
      <Pane />
      <Pane />
    </>
  ));
  try {
    view.getAllByText('Exit reply')[1].click();
    expect(document.activeElement).toBe(view.getAllByTestId('card')[1]);
    view.getAllByText('Exit reply')[0].click();
    expect(document.activeElement).toBe(view.getAllByTestId('card')[0]);
  } finally {
    view.unmount();
    vi.unstubAllGlobals();
  }
});

it('renders the composer for a Gmail-linked thread (unchanged default behavior)', async () => {
  linkFixtures.links = [inboxLink({ id: 'inbox', provider: 'GMAIL' })];
  const first = message('first');
  const view = render(() => (
    <ThreadTestProvider messages={[first]}>
      {() => <ThreadReplyInput replyingTo={() => first} />}
    </ThreadTestProvider>
  ));
  try {
    expect(await view.findByText('first')).toBeTruthy();
    expect(view.queryByText(/read-only/i)).toBeNull();
  } finally {
    view.unmount();
  }
});

it('shows a read-only notice instead of the composer for a Microsoft-linked thread', async () => {
  linkFixtures.links = [
    inboxLink({ id: 'inbox', provider: 'MICROSOFT' as Link['provider'] }),
  ];
  const first = message('first');
  const view = render(() => (
    <ThreadTestProvider messages={[first]}>
      {() => <ThreadReplyInput replyingTo={() => first} />}
    </ThreadTestProvider>
  ));
  try {
    expect(await view.findByText(/read-only/i)).toBeTruthy();
    expect(view.queryByText('first')).toBeNull();
  } finally {
    view.unmount();
  }
});

it("fails closed (shows the read-only notice, not the composer) when the thread's link cannot be resolved at all", async () => {
  linkFixtures.links = [];
  const first = message('first');
  const view = render(() => (
    <ThreadTestProvider messages={[first]}>
      {() => <ThreadReplyInput replyingTo={() => first} />}
    </ThreadTestProvider>
  ));
  try {
    expect(await view.findByText(/read-only/i)).toBeTruthy();
    expect(view.queryByText('first')).toBeNull();
  } finally {
    view.unmount();
  }
});

it('fails closed while the links list is still loading, then renders the composer once a writable Gmail link resolves', async () => {
  const deferred = Promise.withResolvers<{ links: Link[] }>();
  linkFixtures.getLinksOverride = () => deferred.promise;
  const first = message('first');
  const view = render(() => (
    <ThreadTestProvider messages={[first]}>
      {() => <ThreadReplyInput replyingTo={() => first} />}
    </ThreadTestProvider>
  ));
  try {
    // Still pending: never shows the composer while capability is unknown.
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(view.queryByText('first')).toBeNull();

    deferred.resolve({ links: [inboxLink({ id: 'inbox', provider: 'GMAIL' })] });
    expect(await view.findByText('first')).toBeTruthy();
  } finally {
    linkFixtures.getLinksOverride = undefined;
    view.unmount();
  }
});
