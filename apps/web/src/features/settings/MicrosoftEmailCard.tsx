import { toast } from '@core/component/Toast/Toast';
import { useUserId } from '@core/context/user';
import { isMicrosoftProvider } from '@core/email-link/capabilities';
import { useEmailLinks } from '@core/email-link';
import { useAddMicrosoftMailboxFlow } from '@core/email-link/microsoft-mailbox-flow';
import MicrosoftIcon from '@icon/mcp-microsoft365.svg';
import TrashIcon from '@phosphor-icons/core/regular/trash.svg?component-solid';
import { useRemoveInboxMutation } from '@queries/email/link';
import type { Link as EmailLink } from '@service-email/generated/schemas';
import { Button, Dialog, Panel, Tooltip } from '@ui';
import { createMemo, createSignal, For, Show } from 'solid-js';
import { ConnectAction, StatusDot } from './integration-ui';
import { Chip, IntegrationRow, SettingsCard, SettingsRow } from './primitives';

/**
 * Microsoft 365 (Outlook) integration as its own Connected-accounts card,
 * deliberately separate from `EmailCard` (Gmail): each provider gets its own
 * connect action, own inbox rows, and own disconnect, so a user picks Gmail
 * or Microsoft 365 (or both) explicitly rather than one combined flow
 * guessing for them. Every mailbox here is read-only — the backend Microsoft
 * 365 adapter holds a delegated, read-only Microsoft Graph grant per user,
 * not a Microsoft identity/login, and never sends, replies, drafts, marks
 * read, labels, moves, deletes, or shares on the user's behalf (enforced by
 * the backend; see `capabilities.ts` for the UI-side mirror of that rule and
 * `thread-action-adapter.tsx` / `EmailThreadLoadGate.tsx` for where it's
 * applied).
 *
 * Gated by `enableMicrosoft365Email` at the call site (`Email.tsx`) — this
 * component assumes the flag is already on.
 *
 * Real, already-implemented contract: `authServiceClient
 * .initMicrosoftMailboxConnect` → `POST /microsoft-mailbox/connect`, and the
 * mailbox is actually provisioned once the OAuth round trip completes and
 * lands on `MicrosoftMailboxCompletionRoute`
 * (`POST /email/links/microsoft/init`) — not here. This component only
 * starts the redirect and renders whatever the (provider-agnostic) email
 * links list already reports. See docs/AGENT_GUIDE/email-microsoft365.md.
 */
export function MicrosoftEmailCard() {
  const userId = useUserId();
  const { query: emailLinksQuery } = useEmailLinks();
  const startAddMicrosoftMailbox = useAddMicrosoftMailboxFlow();

  const removeInboxMutation = useRemoveInboxMutation({
    onSuccess: () => toast.success('Inbox removed'),
    onError: () => toast.failure('Failed to remove inbox. Please try again.'),
  });

  const [isConnectPending, setIsConnectPending] = createSignal(false);
  const [removeTarget, setRemoveTarget] = createSignal<{
    id: string;
    email: string;
  } | null>(null);

  const microsoftLinks = createMemo(() =>
    (emailLinksQuery.data?.links ?? []).filter((link) =>
      isMicrosoftProvider(link.provider)
    )
  );
  const connected = createMemo(() => microsoftLinks().length > 0);

  const onConnect = async () => {
    if (isConnectPending()) return;
    setIsConnectPending(true);
    try {
      // Navigates the browser away on success; never marks a mailbox
      // connected here — only the server-verified links list does that (see
      // useAddMicrosoftMailboxFlow's doc comment for why).
      await startAddMicrosoftMailbox();
    } finally {
      setIsConnectPending(false);
    }
  };

  const handleRemoveInbox = () => {
    const target = removeTarget();
    if (!target) return;
    setRemoveTarget(null);
    removeInboxMutation.mutate(target.id);
  };

  return (
    <>
      <SettingsCard>
        <IntegrationRow
          icon={<MicrosoftIcon />}
          title="Microsoft 365 (Outlook)"
          description="Read-only. Sending, replies, and mailbox changes stay disabled."
          status={
            <Show when={connected()}>
              <StatusDot state="connected" label="Connected · Read-only" />
            </Show>
          }
        >
          <Show when={!connected()}>
            <ConnectAction
              label="Connect"
              onClick={onConnect}
              disabled={isConnectPending()}
            />
          </Show>
        </IntegrationRow>

        <For each={microsoftLinks()}>
          {(link) => (
            <MicrosoftInboxRow
              link={link}
              isOwn={link.macro_id === userId()}
              onRemove={() =>
                setRemoveTarget({ id: link.id, email: link.email_address })
              }
            />
          )}
        </For>

        <Show when={connected()}>
          <SettingsRow
            label="Add another inbox"
            description="Connect another Microsoft 365 (Outlook) account, read-only."
          >
            <ConnectAction
              label="Connect"
              onClick={onConnect}
              disabled={isConnectPending()}
            />
          </SettingsRow>
        </Show>
      </SettingsCard>

      <Dialog
        open={removeTarget() !== null}
        onOpenChange={(open) => {
          if (!open) setRemoveTarget(null);
        }}
        position="center"
        class="w-120"
      >
        <Panel depth={2} class="rounded-xl">
          <Panel.Header class="px-6">
            <Dialog.Title class="text-ink text-sm font-semibold">
              Remove inbox
            </Dialog.Title>
          </Panel.Header>
          <Panel.Body class="p-6 font-sans flex flex-col gap-3">
            <Dialog.Description class="text-ink-muted text-sm/tight font-normal">
              Remove <span class="text-ink">{removeTarget()?.email}</span>?
              This clears all of its email data from Macro and cannot be
              undone.
            </Dialog.Description>
            <div class="pt-3 justify-end items-center gap-3 inline-flex">
              <Button
                variant="outline"
                depth={3}
                onClick={() => setRemoveTarget(null)}
              >
                Cancel
              </Button>
              <Button variant="danger" depth={3} onClick={handleRemoveInbox}>
                Remove
              </Button>
            </div>
          </Panel.Body>
        </Panel>
      </Dialog>
    </>
  );
}

function MicrosoftInboxRow(props: {
  link: EmailLink;
  isOwn: boolean;
  onRemove: () => void;
}) {
  return (
    <div class="bg-surface flex items-center justify-between gap-3 min-h-15.25 py-2 px-6">
      <div class="min-w-0 flex flex-col gap-0.5">
        <div class="flex items-center gap-2 min-w-0">
          <span class="ph-no-capture text-sm truncate">
            {props.link.email_address}
          </span>
          <Chip label="Read-only" />
          <Show when={!props.isOwn}>
            <Chip label="Shared" />
          </Show>
        </div>
        <span class="text-xs text-ink-muted">
          Macro can only read this mailbox
        </span>
      </div>
      <Tooltip label="Remove inbox">
        <Button
          variant="outline"
          size="icon-sm"
          depth={3}
          onClick={props.onRemove}
          aria-label={`Remove ${props.link.email_address}`}
        >
          <TrashIcon class="size-4" />
        </Button>
      </Tooltip>
    </div>
  );
}
