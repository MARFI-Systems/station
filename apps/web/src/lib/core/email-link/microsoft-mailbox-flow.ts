import { toast } from '@core/component/Toast/Toast';
import { useInitMicrosoftMailboxConnect } from '@queries/auth/microsoft-mailbox-connect';

/**
 * Starts the read-only Microsoft 365 (Outlook) add-inbox flow via the
 * delegated Microsoft Graph OAuth consent screen — explicitly not Microsoft
 * identity/login. Real, already-implemented endpoint: see
 * `authServiceClient.initMicrosoftMailboxConnect` and
 * `services/authentication_service/src/api/microsoft_mailbox.rs`.
 *
 * There is no `original_url`/return-layout stash here (unlike Gmail's
 * `useAddInboxFlow`): the backend's own callback route hardcodes its
 * completion redirect to a single fixed frontend URL
 * (`/settings/connections?microsoftMailbox=connected|denied`, deployment
 * config, not something this call can parameterize), so the user always
 * lands on Settings > Connected accounts after consent regardless of where
 * they started this flow. `MicrosoftMailboxCompletionRoute` handles that
 * landing and is what actually provisions the mailbox
 * (`emailClient.initMicrosoftLink`) once the redirect confirms consent —
 * this function never claims success itself, only kicks off the redirect.
 */
export function useAddMicrosoftMailboxFlow() {
  const initConnect = useInitMicrosoftMailboxConnect();

  return async () => {
    const result = await initConnect.mutateAsync();
    if (result.isErr()) {
      toast.failure('Failed to start Microsoft 365 connection');
      return;
    }
    window.location.href = result.value.authorizationUrl;
  };
}
