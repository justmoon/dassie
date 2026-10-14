/**
 * The dev dashboard (spec 6 §2): a sidebar with the pages and the nodes,
 * and the page. Its data comes from the dev server's control channel;
 * a node's pages talk to that node through the dev server's relay.
 */

import type { JSX } from "@solidjs/web";
import { Errored, Loading, Match, Switch } from "solid-js";
import { Router, Routes } from "@dassie/ui/router";
import { Toaster } from "@dassie/ui/toast";
import { Alert, PageError, Spinner } from "@dassie/ui/ui";
import { BuildState } from "@dassie/rpc/dev";
import { Sidebar } from "./Sidebar.tsx";
import { createDevStore, DevStoreProvider, useDev } from "./store.ts";
import { Dashboard } from "./pages/Dashboard.tsx";
import { Host } from "./pages/Host.tsx";
import { Logs } from "./pages/Logs.tsx";
import { NodePage } from "./pages/NodePage.tsx";
import { Scenarios } from "./pages/Scenarios.tsx";

const ROUTES = [
  { path: "/logs", component: Logs },
  { path: "/scenarios", component: Scenarios },
  { path: "/nodes/host", component: Host },
  { path: "/nodes/:node", component: NodePage },
  { path: "/nodes/:node/*rest", component: NodePage },
];

export default function App(): JSX.Element {
  const store = createDevStore(`wss://${location.host}/control`);
  return (
    <DevStoreProvider value={store}>
      <Toaster>
        <Router>
          <div class="dev-shell">
            <Sidebar />
            <main id="content">
              <Notices />
              <Errored
                fallback={(error, reset) => (
                  <PageError error={error()} reset={reset} />
                )}
              >
                <Loading fallback={<Spinner label="Loading…" />}>
                  <Routes routes={ROUTES} fallback={Dashboard} />
                </Loading>
              </Errored>
            </main>
          </div>
        </Router>
      </Toaster>
    </DevStoreProvider>
  );
}

/** The connection to the dev server, and the node's build. */
function Notices(): JSX.Element {
  const dev = useDev();
  const build = () => dev.state().build;
  return (
    <Switch>
      <Match when={dev.status() !== "open"}>
        <p class="banner" role="status" data-testid="dev-disconnected">
          Connecting to the dev server…
        </p>
      </Match>
      <Match when={build()?.state === BuildState.FAILED}>
        <Alert
          title="The node did not build"
          tone="danger"
          testid="build-failed"
        >
          <p>The nodes keep running the binary built before.</p>
          <pre>{build()?.errorExcerpt}</pre>
        </Alert>
      </Match>
      <Match when={build()?.state === BuildState.BUILDING}>
        <p class="banner" role="status" data-testid="building">
          Building the node…
        </p>
      </Match>
    </Switch>
  );
}
