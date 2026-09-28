import { authServiceClient } from '@service-auth/client';
import { useMutation } from '@tanstack/solid-query';

/**
 * Real, already-implemented endpoint: see
 * `authServiceClient.initMicrosoftMailboxConnect` and
 * `services/authentication_service/src/api/microsoft_mailbox.rs`. Starts the
 * delegated, read-only Microsoft 365 mailbox consent flow for the
 * already-authenticated user — not Microsoft identity/login. Takes no
 * arguments: the backend fixes its own redirect target.
 */
export function useInitMicrosoftMailboxConnect() {
  return useMutation(() => ({
    mutationFn: async () => authServiceClient.initMicrosoftMailboxConnect(),
  }));
}
