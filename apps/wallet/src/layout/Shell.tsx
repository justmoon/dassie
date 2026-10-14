/**
 * The wallet's frame: the navigation, a notice while the connection is
 * down, and the page (spec 6 §1.5's routes, and the consent and connected
 * apps pages of D9; Send and payment status come with M6c).
 */

import type { JSX } from "@solidjs/web";
import {
  createSignal,
  Errored,
  lazy,
  Loading,
  onCleanup,
  Show,
} from "solid-js";
import { Account } from "../pages/Account.tsx";
import { CreateLedger } from "../pages/ledgers/CreateLedger.tsx";
import { DeleteLedger } from "../pages/ledgers/DeleteLedger.tsx";
import { Ledgers } from "../pages/ledgers/Ledgers.tsx";
import { Receive } from "../pages/Receive.tsx";
import { useClient } from "@dassie/ui/rpc";
import { Routes } from "@dassie/ui/router";
import { PageError, Spinner } from "@dassie/ui/ui";
import { MainNav } from "./MainNav.tsx";

const Network = lazy(() => import("../pages/network/Network.tsx"), {
  export: "Network",
});
const Register = lazy(() => import("../pages/network/Register.tsx"), {
  export: "Register",
});
const Settings = lazy(() => import("../pages/settings/Settings.tsx"), {
  export: "Settings",
});
const Activity = lazy(() => import("../pages/payments/Activity.tsx"), {
  export: "Activity",
});
const Consent = lazy(() => import("../pages/grants/Consent.tsx"), {
  export: "Consent",
});
const Grants = lazy(() => import("../pages/grants/Grants.tsx"), {
  export: "Grants",
});
const DebugPage = lazy(() => import("@dassie/ui/debug"), {
  export: "DebugPage",
});

const ROUTES = [
  { path: "/network/register", component: Register },
  { path: "/network/:node", component: Network },
  { path: "/network", component: Network },
  { path: "/ledgers/create", component: CreateLedger },
  { path: "/ledgers/delete/:scheme", component: DeleteLedger },
  { path: "/ledgers", component: Ledgers },
  { path: "/receive", component: Receive },
  { path: "/payments", component: Activity },
  { path: "/consent/:id", component: Consent },
  { path: "/grants", component: Grants },
  { path: "/settings", component: Settings },
  { path: "/debug", component: DebugPage },
  { path: "/debug/*rest", component: DebugPage },
];

export function Shell(): JSX.Element {
  return (
    <div class="shell">
      <a class="skip" href="#content">Skip to content</a>
      <MainNav />
      <ConnectionNotice />
      <main id="content">
        <Errored
          fallback={(error, reset) => (
            <PageError error={error()} reset={reset} />
          )}
        >
          <Loading fallback={<Spinner label="Loading…" />}>
            <Routes routes={ROUTES} fallback={Account} />
          </Loading>
        </Errored>
      </main>
    </div>
  );
}

/** Says so when the connection to the node has been down a moment. */
function ConnectionNotice(): JSX.Element {
  const client = useClient();
  const [down, setDown] = createSignal(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  // Retries pass through "connecting"; only an open connection, or none
  // needed, ends the notice.
  const stop = client.onConnectionChange((state) => {
    if (state === "waiting" && timer === undefined) {
      timer = setTimeout(() => setDown(true), 2_000);
    } else if (state !== "waiting" && state !== "connecting") {
      clearTimeout(timer);
      timer = undefined;
      setDown(false);
    }
  });
  onCleanup(() => {
    stop();
    clearTimeout(timer);
  });
  return (
    <Show when={down()}>
      <p class="banner" role="status" data-testid="connection-banner">
        Reconnecting to your node…
      </p>
    </Show>
  );
}
