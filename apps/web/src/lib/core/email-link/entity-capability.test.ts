import { afterEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  getQueryData: vi.fn(),
}));

vi.mock('@queries/client', () => ({
  queryClient: { getQueryData: mocks.getQueryData },
}));
vi.mock('@queries/email/keys', () => ({
  emailKeys: { links: { queryKey: ['email', 'links'] } },
}));

import { isEmailEntityWritable } from './entity-capability';

afterEach(() => {
  vi.clearAllMocks();
});

describe('isEmailEntityWritable', () => {
  it('is always true for non-email entities regardless of cache state', () => {
    mocks.getQueryData.mockReturnValue(undefined);
    expect(isEmailEntityWritable({ type: 'document' })).toBe(true);
    expect(isEmailEntityWritable({ type: 'reminder' })).toBe(true);
  });

  it('fails closed when the links list has not loaded yet', () => {
    mocks.getQueryData.mockReturnValue(undefined);
    expect(isEmailEntityWritable({ type: 'email', linkId: 'inbox' })).toBe(
      false
    );
  });

  it('resolves an explicit linkId against the loaded links list', () => {
    mocks.getQueryData.mockReturnValue({
      links: [
        { id: 'inbox', is_primary: true, provider: 'GMAIL', is_read_only: false },
        { id: 'ms-inbox', is_primary: false, provider: 'MICROSOFT', is_read_only: true },
      ],
    });
    expect(isEmailEntityWritable({ type: 'email', linkId: 'inbox' })).toBe(
      true
    );
    expect(isEmailEntityWritable({ type: 'email', linkId: 'ms-inbox' })).toBe(
      false
    );
  });

  it('resolves against the primary link when linkId is omitted (the normal primary-inbox case)', () => {
    mocks.getQueryData.mockReturnValue({
      links: [
        { id: 'inbox', is_primary: true, provider: 'GMAIL', is_read_only: false },
      ],
    });
    expect(isEmailEntityWritable({ type: 'email' })).toBe(true);
  });

  it('fails closed when linkId is set but not found in the loaded list', () => {
    mocks.getQueryData.mockReturnValue({ links: [] });
    expect(isEmailEntityWritable({ type: 'email', linkId: 'missing' })).toBe(
      false
    );
  });
});
