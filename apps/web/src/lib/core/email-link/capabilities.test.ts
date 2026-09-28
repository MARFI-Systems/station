import { describe, expect, it } from 'vitest';
import {
  getEmailCapabilities,
  isLinkReadOnly,
  isMicrosoftProvider,
  MICROSOFT_PROVIDER,
} from './capabilities';

describe('isMicrosoftProvider', () => {
  it('is true only for the real Microsoft provider value', () => {
    expect(isMicrosoftProvider(MICROSOFT_PROVIDER)).toBe(true);
    expect(isMicrosoftProvider('MICROSOFT')).toBe(true);
  });

  it('is false for Gmail and unset providers', () => {
    expect(isMicrosoftProvider('GMAIL')).toBe(false);
    expect(isMicrosoftProvider(undefined)).toBe(false);
    expect(isMicrosoftProvider(null)).toBe(false);
  });
});

describe('isLinkReadOnly (fails closed)', () => {
  it('is writable only for a known Gmail link the backend has not marked read-only', () => {
    expect(isLinkReadOnly({ provider: 'GMAIL', is_read_only: false })).toBe(
      false
    );
    expect(isLinkReadOnly({ provider: 'GMAIL' })).toBe(false);
  });

  it('is read-only for Gmail when the backend explicitly says so', () => {
    expect(isLinkReadOnly({ provider: 'GMAIL', is_read_only: true })).toBe(
      true
    );
  });

  it('is unconditionally read-only for Microsoft and never trusts a false is_read_only override', () => {
    expect(isLinkReadOnly({ provider: 'MICROSOFT' })).toBe(true);
    expect(
      isLinkReadOnly({ provider: 'MICROSOFT', is_read_only: false })
    ).toBe(true);
  });

  it('fails closed (read-only) for an unresolved link — unknown or still loading', () => {
    expect(isLinkReadOnly(undefined)).toBe(true);
    expect(isLinkReadOnly(null)).toBe(true);
  });

  it('fails closed for any provider value that is neither GMAIL nor MICROSOFT', () => {
    expect(isLinkReadOnly({ provider: 'SOMETHING_ELSE' })).toBe(true);
  });
});

describe('getEmailCapabilities', () => {
  it('grants full access only for a known writable Gmail link', () => {
    const caps = getEmailCapabilities({ provider: 'GMAIL', is_read_only: false });
    expect(caps).toEqual({
      readOnly: false,
      canSend: true,
      canReply: true,
      canDraft: true,
      canMarkRead: true,
      canLabel: true,
      canMove: true,
      canDelete: true,
      canShare: true,
    });
  });

  it('fails closed to read-only when the link is unresolved, not full access', () => {
    expect(getEmailCapabilities(undefined).readOnly).toBe(true);
    expect(getEmailCapabilities(undefined).canSend).toBe(false);
  });

  it('denies every mutating action for a Microsoft link, including implicit read', () => {
    const caps = getEmailCapabilities({
      provider: 'MICROSOFT',
      is_read_only: true,
    });
    expect(caps).toEqual({
      readOnly: true,
      canSend: false,
      canReply: false,
      canDraft: false,
      canMarkRead: false,
      canLabel: false,
      canMove: false,
      canDelete: false,
      canShare: false,
    });
  });
});
