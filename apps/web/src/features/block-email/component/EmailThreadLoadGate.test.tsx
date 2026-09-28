/**
 * @vitest-environment jsdom
 */

import { render } from '@solidjs/testing-library';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { EmailThreadLoadGate } from './EmailThreadLoadGate';

const mocks = vi.hoisted(() => ({
  links: [] as Array<{ id: string; provider: string; is_read_only?: boolean }>,
  linksLoaded: true,
}));

vi.mock('@core/component/EntityLoadGate', () => ({
  EntityLoadGate: (props: { children: unknown }) => props.children,
}));
vi.mock('@notifications', () => ({
  EmailDebouncedReadMarker: () => (
    <div data-testid="read-marker">read-marker</div>
  ),
}));
vi.mock('@queries/email/link', () => ({
  useEmailLinksQuery: () => ({
    isPending: !mocks.linksLoaded,
    data: mocks.linksLoaded ? { links: mocks.links } : undefined,
  }),
}));

afterEach(() => {
  mocks.links = [];
  mocks.linksLoaded = true;
  vi.clearAllMocks();
});

function gateProps(linkId: string) {
  return {
    result: {
      data: () => undefined,
      error: () => undefined,
      isPending: () => false,
    },
    notificationSource: {} as never,
    threadId: 'thread-1',
    linkId,
    onRetry: () => {},
  };
}

describe('EmailThreadLoadGate implicit read-marker gate', () => {
  it('mounts the read marker for a Gmail-linked thread (unchanged default behavior)', async () => {
    mocks.links = [{ id: 'inbox', provider: 'GMAIL', is_read_only: false }];
    const view = render(() => (
      <EmailThreadLoadGate {...gateProps('inbox')}>
        <div>body</div>
      </EmailThreadLoadGate>
    ));
    try {
      expect(await view.findByTestId('read-marker')).toBeTruthy();
    } finally {
      view.unmount();
    }
  });

  it('never mounts the read marker for a read-only Microsoft-linked thread', async () => {
    mocks.links = [{ id: 'inbox', provider: 'MICROSOFT', is_read_only: true }];
    const view = render(() => (
      <EmailThreadLoadGate {...gateProps('inbox')}>
        <div>body</div>
      </EmailThreadLoadGate>
    ));
    try {
      expect(await view.findByText('body')).toBeTruthy();
      expect(view.queryByTestId('read-marker')).toBeNull();
    } finally {
      view.unmount();
    }
  });

  it('fails closed (no read marker) while the links list is still loading/unknown', async () => {
    mocks.linksLoaded = false;
    const view = render(() => (
      <EmailThreadLoadGate {...gateProps('inbox')}>
        <div>body</div>
      </EmailThreadLoadGate>
    ));
    try {
      expect(await view.findByText('body')).toBeTruthy();
      expect(view.queryByTestId('read-marker')).toBeNull();
    } finally {
      view.unmount();
    }
  });
});
