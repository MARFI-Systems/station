/**
 * @vitest-environment jsdom
 */

import { render } from '@solidjs/testing-library';
import { ok, err } from 'neverthrow';
import { afterEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  navigate: vi.fn(),
  searchParams: {} as Record<string, string | undefined>,
  initMicrosoftLink: vi.fn(),
  invalidateEmailLinks: vi.fn(),
  toastSuccess: vi.fn(),
  toastFailure: vi.fn(),
}));

vi.mock('@app/constants/defaultRoute', () => ({ DEFAULT_ROUTE: '' }));
vi.mock('@core/component/LoadingBlock', () => ({
  LoadingBlock: () => <div data-testid="loading">loading</div>,
}));
vi.mock('@core/component/Toast/Toast', () => ({
  toast: { success: mocks.toastSuccess, failure: mocks.toastFailure },
}));
vi.mock('@core/constant/settingsTabsConfig', () => ({
  settingsTabToSlug: () => 'connected',
}));
vi.mock('@queries/email/link', () => ({
  invalidateEmailLinks: mocks.invalidateEmailLinks,
}));
vi.mock('@service-email/client', () => ({
  emailClient: { initMicrosoftLink: mocks.initMicrosoftLink },
}));
vi.mock('@solidjs/router', () => ({
  useNavigate: () => mocks.navigate,
  useSearchParams: () => [mocks.searchParams],
}));

import {
  MICROSOFT_MAILBOX_COMPLETION_PATH,
  MicrosoftMailboxCompletionRoute,
} from './MicrosoftMailboxCompletionRoute';

afterEach(() => {
  mocks.searchParams = {};
  vi.clearAllMocks();
});

describe('MicrosoftMailboxCompletionRoute', () => {
  it('provisions the mailbox via server-verified init on ?microsoftMailbox=connected, never trusting the query param alone', async () => {
    mocks.searchParams = { microsoftMailbox: 'connected' };
    mocks.initMicrosoftLink.mockResolvedValue(
      ok({ linkId: 'link-1', email: 'person@contoso.com', created: true })
    );

    render(() => <MicrosoftMailboxCompletionRoute />);
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(mocks.initMicrosoftLink).toHaveBeenCalledTimes(1);
    expect(mocks.invalidateEmailLinks).toHaveBeenCalledTimes(1);
    expect(mocks.toastSuccess).toHaveBeenCalledWith(
      'Connected person@contoso.com'
    );
    expect(mocks.navigate).toHaveBeenCalledWith(
      expect.stringContaining('connected'),
      expect.objectContaining({ replace: true })
    );
  });

  it("surfaces a failure and provisions nothing when the backend's own re-verification (409) disagrees with the query param", async () => {
    mocks.searchParams = { microsoftMailbox: 'connected' };
    mocks.initMicrosoftLink.mockResolvedValue(
      err([{ code: 'HTTP_ERROR', message: 'not connected' }])
    );

    render(() => <MicrosoftMailboxCompletionRoute />);
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(mocks.invalidateEmailLinks).not.toHaveBeenCalled();
    expect(mocks.toastFailure).toHaveBeenCalled();
    expect(mocks.navigate).toHaveBeenCalled();
  });

  it('never calls init on ?microsoftMailbox=denied', async () => {
    mocks.searchParams = { microsoftMailbox: 'denied' };

    render(() => <MicrosoftMailboxCompletionRoute />);
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(mocks.initMicrosoftLink).not.toHaveBeenCalled();
    expect(mocks.toastFailure).toHaveBeenCalled();
    expect(mocks.navigate).toHaveBeenCalled();
  });

  // This route's own path, `/settings/connections`, is *also* the real
  // "Connected" settings tab's split-router URL (see the collision test
  // below) — a direct/bookmarked visit with no OAuth query string is
  // therefore expected and must not be treated as an error, must not touch
  // the backend, and above all must redirect to a genuinely different
  // top-level path so it can never re-match this same route (an infinite
  // redirect).
  it('redirects once to a different path and touches nothing else when landed on directly with no OAuth query param', async () => {
    mocks.searchParams = {};

    render(() => <MicrosoftMailboxCompletionRoute />);
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(mocks.initMicrosoftLink).not.toHaveBeenCalled();
    expect(mocks.invalidateEmailLinks).not.toHaveBeenCalled();
    expect(mocks.toastFailure).not.toHaveBeenCalled();
    expect(mocks.toastSuccess).not.toHaveBeenCalled();
    expect(mocks.navigate).toHaveBeenCalledTimes(1);

    const [target] = mocks.navigate.mock.calls[0] as [string];
    expect(target).not.toBe(MICROSOFT_MAILBOX_COMPLETION_PATH);
  });

  it('redirects once (no loop) for an unrecognized ?microsoftMailbox value the same way as no param at all', async () => {
    mocks.searchParams = { microsoftMailbox: 'something-unexpected' };

    render(() => <MicrosoftMailboxCompletionRoute />);
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(mocks.initMicrosoftLink).not.toHaveBeenCalled();
    expect(mocks.toastFailure).not.toHaveBeenCalled();
    expect(mocks.navigate).toHaveBeenCalledTimes(1);
    const [target] = mocks.navigate.mock.calls[0] as [string];
    expect(target).not.toBe(MICROSOFT_MAILBOX_COMPLETION_PATH);
  });

  // Pins the exact collision this route has to route around. If either side
  // ever drifts (the real settings slug changes, or this route's path
  // changes) this test forces a conscious update instead of a silent regap.
  it('documents the real routing collision: "Connected" settings tab slug is literally "connections", the same final segment as this route\'s own path', async () => {
    const real = await vi.importActual<
      typeof import('@core/constant/settingsTabsConfig')
    >('@core/constant/settingsTabsConfig');

    expect(real.settingsTabToSlug('Connected')).toBe('connections');
    expect(MICROSOFT_MAILBOX_COMPLETION_PATH).toBe(
      `/settings/${real.settingsTabToSlug('Connected')}`
    );
  });
});
