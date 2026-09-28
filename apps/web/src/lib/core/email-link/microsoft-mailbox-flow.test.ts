import { err, ok } from 'neverthrow';
import { afterEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  mutateAsync: vi.fn(),
  toastFailure: vi.fn(),
}));

vi.mock('@queries/auth/microsoft-mailbox-connect', () => ({
  useInitMicrosoftMailboxConnect: () => ({ mutateAsync: mocks.mutateAsync }),
}));
vi.mock('@core/component/Toast/Toast', () => ({
  toast: { failure: mocks.toastFailure, success: vi.fn() },
}));

import { useAddMicrosoftMailboxFlow } from './microsoft-mailbox-flow';

afterEach(() => {
  vi.clearAllMocks();
});

describe('useAddMicrosoftMailboxFlow', () => {
  it('redirects to the authorizationUrl, never claiming success itself', async () => {
    mocks.mutateAsync.mockResolvedValue(
      ok({ authorizationUrl: 'https://login.microsoftonline.com/consent' })
    );
    const startFlow = useAddMicrosoftMailboxFlow();

    const originalLocation = window.location;
    // @ts-expect-error overriding for the assertion
    delete window.location;
    // @ts-expect-error partial mock is enough for this assertion
    window.location = { ...originalLocation, href: '' };

    await startFlow();

    expect(window.location.href).toBe(
      'https://login.microsoftonline.com/consent'
    );
    expect(mocks.toastFailure).not.toHaveBeenCalled();

    window.location = originalLocation;
  });

  it('surfaces a failure toast and never navigates when the backend rejects the request', async () => {
    mocks.mutateAsync.mockResolvedValue(
      err([{ code: 'HTTP_ERROR', message: 'boom' }])
    );
    const originalHref = window.location.href;
    const startFlow = useAddMicrosoftMailboxFlow();

    await startFlow();

    expect(mocks.toastFailure).toHaveBeenCalledWith(
      'Failed to start Microsoft 365 connection'
    );
    expect(window.location.href).toBe(originalHref);
  });
});
