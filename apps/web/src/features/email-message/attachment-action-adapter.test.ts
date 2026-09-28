import { ok, err } from 'neverthrow';
import { createRoot, createSignal } from 'solid-js';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { EmailAttachment } from './core/email-message';

const mocks = vi.hoisted(() => ({
  openWithSplit: vi.fn(),
  getAttachmentUrl: vi.fn(),
  getEmailAttachmentDocument: vi.fn(),
  getEmailAttachmentMetadata: vi.fn(),
  refetchSoupEntity: vi.fn(),
  toastFailure: vi.fn(),
  telemetryError: vi.fn(),
  windowOpen: vi.fn(),
  links: [] as Array<{ id: string; provider: string; is_read_only?: boolean }>,
  linksLoaded: true,
}));

vi.mock('@components/app/split-layout/layout', () => ({
  useSplitLayout: () => ({ openWithSplit: mocks.openWithSplit }),
}));
vi.mock('@core/component/Toast/Toast', () => ({
  toast: { failure: mocks.toastFailure, success: vi.fn() },
}));
vi.mock('@core/constant/allBlocks', () => ({
  fileTypeToBlockName: () => 'pdf',
}));
vi.mock('@macro-inc/observability', () => ({
  Telemetry: { error: mocks.telemetryError },
}));
vi.mock('@queries/email/integration', () => ({
  getEmailAttachmentDocument: mocks.getEmailAttachmentDocument,
  getEmailAttachmentMetadata: mocks.getEmailAttachmentMetadata,
}));
vi.mock('@queries/email/link', () => ({
  useEmailLinksQuery: () => ({
    isPending: !mocks.linksLoaded,
    data: mocks.linksLoaded ? { links: mocks.links } : undefined,
  }),
}));
vi.mock('@queries/soup/cache', () => ({
  refetchSoupEntity: mocks.refetchSoupEntity,
}));
vi.mock('@service-email/client', () => ({
  emailClient: { getAttachmentUrl: mocks.getAttachmentUrl },
}));
vi.mock('@service-storage/fileTypeMap', () => ({
  FileTypeMap: { pdf: { mime: 'application/pdf', extension: 'pdf' } },
}));

import { createEmailAttachmentOpener } from './attachment-action-adapter';

const attachment = (overrides: Partial<EmailAttachment> = {}): EmailAttachment => ({
  db_id: 'att-1',
  filename: 'file.pdf',
  mime_type: 'application/pdf',
  ...overrides,
});

function runOpener(linkId: string | undefined) {
  const [threadLink] = createSignal(linkId ? { link_id: linkId } : undefined);
  let openAttachment!: ReturnType<typeof createEmailAttachmentOpener>;
  const dispose = createRoot((d) => {
    openAttachment = createEmailAttachmentOpener(threadLink);
    return d;
  });
  return { openAttachment, dispose };
}

afterEach(() => {
  mocks.links = [];
  mocks.linksLoaded = true;
  vi.clearAllMocks();
  vi.unstubAllGlobals();
});

describe('createEmailAttachmentOpener — Gmail (existing document-open behavior, unchanged)', () => {
  it('converts to a Macro document and opens it in the split view', async () => {
    mocks.links = [{ id: 'inbox', provider: 'GMAIL', is_read_only: false }];
    mocks.getEmailAttachmentDocument.mockResolvedValue(
      ok({ attachment_id: 'att-1', document_id: 'doc-1' })
    );
    mocks.getEmailAttachmentMetadata.mockResolvedValue(ok({}));

    const { openAttachment, dispose } = runOpener('inbox');
    await openAttachment(attachment());

    expect(mocks.getEmailAttachmentDocument).toHaveBeenCalledWith('att-1');
    expect(mocks.getEmailAttachmentMetadata).toHaveBeenCalledWith('doc-1');
    expect(mocks.refetchSoupEntity).toHaveBeenCalledWith('doc-1', 'document');
    expect(mocks.openWithSplit).toHaveBeenCalledWith(
      { type: 'pdf', id: 'doc-1' },
      { preferNewSplit: true }
    );
    expect(mocks.getAttachmentUrl).not.toHaveBeenCalled();
    dispose();
  });

  it('falls back to the Gmail path when the link is unresolved/still loading, never routing an unconfirmed thread to the Microsoft path', async () => {
    mocks.linksLoaded = false;
    mocks.getEmailAttachmentDocument.mockResolvedValue(
      ok({ attachment_id: 'att-1', document_id: 'doc-1' })
    );
    mocks.getEmailAttachmentMetadata.mockResolvedValue(ok({}));

    const { openAttachment, dispose } = runOpener('inbox');
    await openAttachment(attachment());

    expect(mocks.getEmailAttachmentDocument).toHaveBeenCalledWith('att-1');
    expect(mocks.getAttachmentUrl).not.toHaveBeenCalled();
    dispose();
  });

  it('reports failure and never opens anything when document creation fails', async () => {
    mocks.links = [{ id: 'inbox', provider: 'GMAIL', is_read_only: false }];
    mocks.getEmailAttachmentDocument.mockResolvedValue(
      err([{ code: 'HTTP_ERROR', message: 'boom' }])
    );

    const { openAttachment, dispose } = runOpener('inbox');
    await openAttachment(attachment());

    expect(mocks.toastFailure).toHaveBeenCalled();
    expect(mocks.openWithSplit).not.toHaveBeenCalled();
    dispose();
  });
});

describe('createEmailAttachmentOpener — Microsoft (direct presigned-URL path)', () => {
  it('uses GET /email/attachments/{id} directly and opens the presigned https data_url, never the document_id endpoint', async () => {
    mocks.links = [{ id: 'ms-inbox', provider: 'MICROSOFT', is_read_only: true }];
    mocks.getAttachmentUrl.mockResolvedValue(
      ok({
        attachment: {
          db_id: 'att-1',
          data_url: 'https://d111111abcdef8.cloudfront.net/temp/link/att-1-file.pdf?Signature=xyz',
        },
      })
    );
    const openSpy = vi.fn();
    vi.stubGlobal('open', openSpy);

    const { openAttachment, dispose } = runOpener('ms-inbox');
    await openAttachment(attachment());

    expect(mocks.getAttachmentUrl).toHaveBeenCalledWith({ id: 'att-1' });
    expect(mocks.getEmailAttachmentDocument).not.toHaveBeenCalled();
    expect(openSpy).toHaveBeenCalledWith(
      'https://d111111abcdef8.cloudfront.net/temp/link/att-1-file.pdf?Signature=xyz',
      '_blank',
      'noopener,noreferrer'
    );
    dispose();
  });

  it('never exposes a Graph token: only the presigned https URL reaches window.open, nothing bearer/token-shaped', async () => {
    mocks.links = [{ id: 'ms-inbox', provider: 'MICROSOFT', is_read_only: true }];
    mocks.getAttachmentUrl.mockResolvedValue(
      ok({
        attachment: {
          db_id: 'att-1',
          data_url: 'https://cdn.example.com/att-1',
        },
      })
    );
    const openSpy = vi.fn();
    vi.stubGlobal('open', openSpy);

    const { openAttachment, dispose } = runOpener('ms-inbox');
    await openAttachment(attachment());

    expect(openSpy).toHaveBeenCalledTimes(1);
    const [openedUrl] = openSpy.mock.calls[0] as [string];
    expect(openedUrl).not.toMatch(/access_token|bearer|graph\.microsoft\.com/i);
    expect(openedUrl.startsWith('https://')).toBe(true);
    dispose();
  });

  it('refuses to open an unsafe (non-https) data_url and never calls window.open', async () => {
    mocks.links = [{ id: 'ms-inbox', provider: 'MICROSOFT', is_read_only: true }];
    mocks.getAttachmentUrl.mockResolvedValue(
      ok({ attachment: { db_id: 'att-1', data_url: 'javascript:alert(1)' } })
    );
    const openSpy = vi.fn();
    vi.stubGlobal('open', openSpy);

    const { openAttachment, dispose } = runOpener('ms-inbox');
    await openAttachment(attachment());

    expect(openSpy).not.toHaveBeenCalled();
    expect(mocks.toastFailure).toHaveBeenCalled();
    dispose();
  });

  it('refuses to open a missing data_url and never calls window.open', async () => {
    mocks.links = [{ id: 'ms-inbox', provider: 'MICROSOFT', is_read_only: true }];
    mocks.getAttachmentUrl.mockResolvedValue(
      ok({ attachment: { db_id: 'att-1', data_url: null } })
    );
    const openSpy = vi.fn();
    vi.stubGlobal('open', openSpy);

    const { openAttachment, dispose } = runOpener('ms-inbox');
    await openAttachment(attachment());

    expect(openSpy).not.toHaveBeenCalled();
    expect(mocks.toastFailure).toHaveBeenCalled();
    dispose();
  });

  it('reports failure and never opens anything when the backend call fails', async () => {
    mocks.links = [{ id: 'ms-inbox', provider: 'MICROSOFT', is_read_only: true }];
    mocks.getAttachmentUrl.mockResolvedValue(
      err([{ code: 'HTTP_ERROR', message: 'boom' }])
    );
    const openSpy = vi.fn();
    vi.stubGlobal('open', openSpy);

    const { openAttachment, dispose } = runOpener('ms-inbox');
    await openAttachment(attachment());

    expect(openSpy).not.toHaveBeenCalled();
    expect(mocks.toastFailure).toHaveBeenCalled();
    dispose();
  });
});
