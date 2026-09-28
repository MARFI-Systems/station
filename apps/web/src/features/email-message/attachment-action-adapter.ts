import { useSplitLayout } from '@components/app/split-layout/layout';
import { toast } from '@core/component/Toast/Toast';
import { fileTypeToBlockName } from '@core/constant/allBlocks';
import { isMicrosoftProvider } from '@core/email-link/capabilities';
import { Telemetry } from '@macro-inc/observability';
import {
  getEmailAttachmentDocument,
  getEmailAttachmentMetadata,
} from '@queries/email/integration';
import { useEmailLinksQuery } from '@queries/email/link';
import { queryReadyGate } from '@queries/gate';
import { refetchSoupEntity } from '@queries/soup/cache';
import { emailClient } from '@service-email/client';
import { FileTypeMap } from '@service-storage/fileTypeMap';
import type { FileType } from '@service-storage/generated/schemas/fileType';
import type { Accessor } from 'solid-js';
import type { EmailAttachment } from './core/email-message';

/**
 * A presigned attachment URL must be `https:` before the browser ever
 * navigates to it — this is the only check available client-side (the
 * CloudFront distribution host is deployment config, not something to
 * hardcode here), but it rules out `javascript:`, `data:`, `blob:`, `http:`,
 * and a malformed/empty value outright. The backend never returns anything
 * else in practice (see `services/email_service/src/api/email/attachments/get.rs`,
 * `get_presigned_url`), so this is a defensive floor, not the real
 * validation.
 */
function isSafeAttachmentUrl(
  url: string | null | undefined
): url is string {
  if (!url) return false;
  try {
    return new URL(url).protocol === 'https:';
  } catch {
    return false;
  }
}

/**
 * Opens a read-only Microsoft attachment directly, bypassing the Macro
 * document conversion pipeline entirely. `GET /email/attachments/{id}/document_id`
 * (`getEmailAttachmentDocument`, used by the Gmail path below) intentionally
 * rejects Microsoft links with 400 `ProviderReadOnly` — see
 * `services/email_service/src/api/email/attachments/get_document_id.rs`:
 * "reject Microsoft before entering the Gmail-backed SFS/DSS ingestion
 * helper." That endpoint uploads the attachment into Macro's own document
 * storage (SFS/DSS) for in-app viewing/editing, which is out of scope for a
 * read-only Microsoft grant.
 *
 * The endpoint that does work for both providers is `GET
 * /email/attachments/{id}` (`emailClient.getAttachmentUrl`,
 * `get.rs`/`GetAttachmentResponse`): it fetches the attachment bytes from
 * whichever provider owns the link (Gmail or Microsoft Graph, server-side —
 * no token of either kind ever reaches the browser), uploads them to S3, and
 * returns a short-lived presigned CloudFront `data_url`. That URL is plain
 * HTTPS static-asset content with no auth of its own, so opening it in a new
 * tab is safe and requires no Macro document representation.
 */
async function openMicrosoftAttachment(attachment: EmailAttachment) {
  const dbId = attachment.db_id;
  if (!dbId) return;

  const response = await emailClient.getAttachmentUrl({ id: dbId });
  if (response.isErr()) {
    toast.failure('Failed to get attachment. Please try again.');
    return Telemetry.error(
      new Error('Failed to get Microsoft attachment url: ' + response.error)
    );
  }

  const dataUrl = response.value.attachment.data_url;
  if (!isSafeAttachmentUrl(dataUrl)) {
    toast.failure('Failed to get attachment. Please try again.');
    return Telemetry.error(
      new Error('Microsoft attachment response had no safe https data_url')
    );
  }

  // A new, unrelated browsing context: no `window.opener` back-reference
  // (`noopener`) and no Referer header carrying the Macro URL to the
  // presigned-URL host (`noreferrer`).
  window.open(dataUrl, '_blank', 'noopener,noreferrer');
}

/** The existing Gmail behavior: convert into a Macro document and open it
 *  in the split view. Unchanged from before this file became provider-aware. */
async function openGmailAttachment(
  attachment: EmailAttachment,
  openWithSplit: ReturnType<typeof useSplitLayout>['openWithSplit']
) {
  const dbId = attachment.db_id;
  if (!dbId) return;
  const response = await getEmailAttachmentDocument(dbId);
  if (response.isErr()) {
    toast.failure('Failed to get attachment. Please try again.');
    return Telemetry.error(
      new Error(
        'Failed to get or create attachment document id: ' + response.error
      )
    );
  }
  const { document_id } = response.value;

  const maybeDocumentMetadata = await getEmailAttachmentMetadata(document_id);
  if (maybeDocumentMetadata.isErr()) {
    toast.failure('Failed to get attachment. Please try again.');
    return Telemetry.error(
      new Error(
        'Failed to get or create attachment document metadata: ' +
          maybeDocumentMetadata.error
      )
    );
  }

  refetchSoupEntity(document_id, 'document');

  const fileType = Object.values(FileTypeMap).findLast(
    (type) => type.mime === attachment.mime_type
  )?.extension;
  const blockName = fileType
    ? fileTypeToBlockName(fileType as FileType)
    : 'unknown';
  openWithSplit({ type: blockName, id: document_id }, { preferNewSplit: true });
}

/**
 * `threadLink` resolves the open thread's own link (not the primary/active
 * one), the same way `thread-action-adapter.tsx` and `thread-reply-input.tsx`
 * do, so a Microsoft attachment on a secondary/delegated inbox routes
 * correctly too. Deliberately does **not** fail closed to the Microsoft path
 * on an unresolved/loading link — unlike the mutation capability guards,
 * routing the wrong way here has asymmetric cost: an unresolved Gmail
 * attachment would otherwise transiently lose its (working, existing)
 * document-open behavior. The Gmail path is the default and only the
 * Microsoft path requires a **positive** provider match.
 */
export function createEmailAttachmentOpener(
  threadLink?: Accessor<{ link_id: string } | undefined>
) {
  const { openWithSplit } = useSplitLayout();
  const emailLinksQuery = useEmailLinksQuery();

  const openAttachment = async (attachment: EmailAttachment) => {
    const linkId = threadLink?.()?.link_id;
    const link =
      linkId && queryReadyGate(emailLinksQuery)
        ? emailLinksQuery.data.links.find((l) => l.id === linkId)
        : undefined;

    if (isMicrosoftProvider(link?.provider)) {
      return openMicrosoftAttachment(attachment);
    }
    return openGmailAttachment(attachment, openWithSplit);
  };

  return openAttachment;
}
