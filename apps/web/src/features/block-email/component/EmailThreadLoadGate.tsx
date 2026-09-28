import { ContentLoading } from '@components/app/ContentLoading';
import {
  EntityLoadGate,
  type EntityLoadResult,
} from '@core/component/EntityLoadGate';
import { getEmailCapabilities } from '@core/email-link/capabilities';
import { EmailDebouncedReadMarker } from '@notifications';
import { useEmailLinksQuery } from '@queries/email/link';
import { queryReadyGate } from '@queries/gate';
import type { ComponentProps, ParentProps } from 'solid-js';
import { createMemo, Show, Suspense } from 'solid-js';

export type EmailThreadLoadGateProps<Data> = ParentProps<{
  result: EntityLoadResult<Data>;
  notificationSource: ComponentProps<
    typeof EmailDebouncedReadMarker
  >['notificationSource'];
  threadId: string;
  linkId?: string;
  debounceTime?: number;
  onRetry: () => void;
}>;

/** Shared load and read-state policy for every mounted email detail host. */
export function EmailThreadLoadGate<Data>(
  props: EmailThreadLoadGateProps<Data>
) {
  // Frontend-side UX guess only (see getEmailCapabilities) — the backend is
  // the final enforcer. Opening a thread otherwise implicitly marks it read
  // once EmailDebouncedReadMarker's debounce elapses; for a read-only
  // (currently: Microsoft) link that implicit write must not happen at all,
  // matching the explicit mark-read/unread guard in thread-action-adapter.tsx.
  const emailLinksQuery = useEmailLinksQuery();
  const canMarkRead = createMemo(() => {
    const link = queryReadyGate(emailLinksQuery)
      ? emailLinksQuery.data.links.find((l) => l.id === props.linkId)
      : undefined;
    return getEmailCapabilities(link).canMarkRead;
  });

  return (
    <Suspense fallback={<ContentLoading />}>
      <EntityLoadGate
        result={props.result}
        loadErrorTitle="Unable to load this email"
        onRetry={props.onRetry}
      >
        <Show when={canMarkRead()}>
          <EmailDebouncedReadMarker
            notificationSource={props.notificationSource}
            threadId={props.threadId}
            linkId={props.linkId}
            debounceTime={props.debounceTime}
          />
        </Show>
        {props.children}
      </EntityLoadGate>
    </Suspense>
  );
}
