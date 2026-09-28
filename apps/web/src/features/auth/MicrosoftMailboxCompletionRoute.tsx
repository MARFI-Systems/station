import { DEFAULT_ROUTE } from '@app/constants/defaultRoute';
import { LoadingBlock } from '@core/component/LoadingBlock';
import { toast } from '@core/component/Toast/Toast';
import { settingsTabToSlug } from '@core/constant/settingsTabsConfig';
import { invalidateEmailLinks } from '@queries/email/link';
import { emailClient } from '@service-email/client';
import { useNavigate, useSearchParams } from '@solidjs/router';
import { onMount } from 'solid-js';

/**
 * Fixed path auth-service's own `/microsoft-mailbox/callback` 303-redirects
 * the browser to after Microsoft consent (deployment config
 * `microsoft_mailbox_completion_url`; see
 * `services/authentication_service/src/api/microsoft_mailbox.rs`,
 * `completion_redirect`). Registered as a top-level route the same way
 * Gmail's `CALLBACK_PATH`/`LINK_CALLBACK_PATH` are (outside the app layout).
 *
 * This exact path is **also** the split-router's own URL for the real
 * "Connected" settings tab: `SETTINGS_TAB_SLUGS.Connected === 'connections'`
 * (`core/constant/settingsTabsConfig.tsx`), and `features/settings/route.tsx`
 * claims `settings/:tab?` under `LAYOUT_ROUTE`'s `/*splits` splat — so a bare
 * `/settings/connections` visit is a legitimate, independently-reachable URL
 * for that panel, not something invented for this feature. Solid-router
 * ranks this route's fully-static path above `LAYOUT_ROUTE`'s splat (the
 * same ranking that already lets Gmail's static `CALLBACK_PATH`/
 * `LINK_CALLBACK_PATH` win over it), so this component — not the settings
 * panel — always renders first for that URL. The `onMount` below closes
 * that gap deliberately: with no (or an unrecognized) `?microsoftMailbox=`
 * value, it does no backend work and redirects immediately to the real
 * settings URL, which is a categorically different top-level path
 * (`${DEFAULT_ROUTE}/~/settings/...`, not `/settings/...`) and therefore
 * cannot re-match this route — see the routing-collision tests in
 * `MicrosoftMailboxCompletionRoute.test.tsx`.
 */
export const MICROSOFT_MAILBOX_COMPLETION_PATH = '/settings/connections';

const CONNECTED_SETTINGS_ROUTE = `${DEFAULT_ROUTE}/~/settings/${settingsTabToSlug('Connected')}`;

/**
 * Lands here after the Microsoft 365 mailbox consent round trip. There is no
 * `link_id` or other value to read from the query string the way Gmail's
 * `/inbox-link-callback` does — only `?microsoftMailbox=connected|denied`,
 * which is a hint to try, not proof of anything. On `connected`, this calls
 * `POST /email/links/microsoft/init` (`emailClient.initMicrosoftLink`, no
 * request body), which independently re-verifies the grant against
 * auth-service's own record server-side before provisioning anything — so
 * even a forged or stale `connected` query param cannot provision a mailbox
 * that isn't actually authorized; the backend answers 409 instead. On
 * `denied`, nothing is provisioned. Either way, this always lands on
 * Settings > Connected accounts — the backend's fixed redirect target means
 * there is no "return to where the user started" the way Gmail's stashed
 * layout return provides (see `useAddMicrosoftMailboxFlow`).
 */
function MicrosoftMailboxCompletionRouteInner() {
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();

  onMount(async () => {
    const result = searchParams.microsoftMailbox;

    if (result === 'connected') {
      const init = await emailClient.initMicrosoftLink();
      if (init.isOk()) {
        invalidateEmailLinks();
        toast.success(`Connected ${init.value.email}`);
      } else {
        toast.failure(
          'Microsoft 365 authorization was not completed. Please try again.'
        );
      }
    } else if (result === 'denied') {
      toast.failure('Microsoft 365 connection was cancelled.');
    }
    // Any other/missing value falls through silently — there's nothing else
    // this route could mean, and the landing is the same either way.

    navigate(CONNECTED_SETTINGS_ROUTE, { replace: true });
  });

  return <LoadingBlock />;
}

export function MicrosoftMailboxCompletionRoute() {
  return <MicrosoftMailboxCompletionRouteInner />;
}
