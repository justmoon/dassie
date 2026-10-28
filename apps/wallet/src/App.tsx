/**
 * The gate (spec/areas/wallet.md#the-gate): which screen the wallet shows
 * follows from the node's state, as `getBasicState` reports it to this
 * connection.
 *
 * - uninitialized: the setup wizard at `/setup/<token>`, otherwise a note
 *   on where to find that URL;
 * - anonymous: the login page, whatever the URL;
 * - authenticated: the forced choice of a first ledger without one, the
 *   wallet otherwise.
 */

import { RpcClient } from "@dassie/rpc";
import type { GetBasicStateResponse } from "@dassie/rpc/proto";
import type { JSX } from "@solidjs/web";
import { createSignal, Errored, lazy, Loading, Match, Switch } from "solid-js";
import { decideScreen, isSetupPath, ownerScreen } from "./gate.ts";
import { Shell } from "./layout/Shell.tsx";
import { Onboarding } from "./pages/ledgers/CreateLedger.tsx";
import { createNodeData, NodeDataContext } from "@dassie/ui/node-data";
import { RpcContext } from "@dassie/ui/rpc";
import { Router } from "@dassie/ui/router";
import { Toaster } from "@dassie/ui/toast";
import { PageError } from "@dassie/ui/ui";
import { Card, Waiting } from "./ui.tsx";
import { realmOf } from "./phrase.ts";
import { UNKNOWN_REALM } from "./messages.ts";

// The setup and login screens, with the identity WASM, load only when
// needed.
const Setup = lazy(() => import("./onboard/Setup.tsx"), { export: "Setup" });
const Login = lazy(() => import("./onboard/Login.tsx"), { export: "Login" });

/** The wallet: asks the node for its state, then shows the matching screen. */
export default function App(): JSX.Element {
  const client = new RpcClient({
    url: `wss://${location.host}/rpc`,
    // The node ended this browser's session (logged out elsewhere, or
    // revoked): reloading shows the login page.
    onSessionEnded: () => location.reload(),
  });
  const [basic, setBasic] = createSignal<GetBasicStateResponse>();
  const [failure, setFailure] = createSignal<string>();
  client.call("getBasicState", {}).then(
    (state) => setBasic(state),
    (error: unknown) =>
      setFailure(error instanceof Error ? error.message : String(error)),
  );
  const screen = () => decideScreen(basic(), failure(), location.pathname);
  // The realm the node reports: a phrase of the other is refused at setup
  // and at login (spec/decisions/phrase-format.md).
  const realm = () => {
    const state = basic();
    return state && realmOf(state.ilpAllocationScheme);
  };
  const asksPhrase = () =>
    screen().kind === "setup" || screen().kind === "login";

  return (
    <RpcContext value={client}>
      <Toaster>
        <Errored
          fallback={(error, reset) => (
            <PageError error={error()} reset={reset} />
          )}
        >
          <Loading fallback={<Waiting label="Loading…" />}>
            <Switch
              fallback={
                <Card title="Unexpected node state">
                  <p>
                    Your Dassie node reported a state this wallet does not know.
                    Is the wallet older than the node?
                  </p>
                </Card>
              }
            >
              <Match when={screen().kind === "loading"}>
                <Waiting label="Connecting to your node…" />
              </Match>
              <Match when={screen().kind === "unreachable"}>
                <Unreachable message={failure() ?? ""} />
              </Match>
              <Match when={screen().kind === "setup-info"}>
                <SetupInfo />
              </Match>
              <Match when={asksPhrase() && realm() === undefined}>
                <Card title="Unknown network">
                  <p data-testid="unknown-realm">{UNKNOWN_REALM}</p>
                </Card>
              </Match>
              <Match when={screen().kind === "setup" && realm()}>
                {(known) => (
                  <Setup
                    token={(screen() as { token: string }).token}
                    realm={known()}
                  />
                )}
              </Match>
              <Match when={screen().kind === "login" && realm()}>
                {(known) => <Login realm={known()} />}
              </Match>
              <Match when={screen().kind === "owner" && basic()}>
                {(state) => <Owner basic={state()} />}
              </Match>
            </Switch>
          </Loading>
        </Errored>
      </Toaster>
    </RpcContext>
  );
}

/** The signed-in owner's wallet, over the node's live data. */
function Owner(props: { basic: GetBasicStateResponse }): JSX.Element {
  // A setup URL means nothing once the node is set up.
  if (isSetupPath(location.pathname)) history.replaceState(null, "", "/");
  const data = createNodeData({
    nodeId: props.basic.nodeId,
    ilpAllocationScheme: props.basic.ilpAllocationScheme,
    hostname: props.basic.hostname,
    version: props.basic.version,
  });
  const screen = () => ownerScreen(data.ledgers());
  return (
    <NodeDataContext value={data}>
      <Router>
        <Switch>
          <Match when={data.ledgersStatus().kind === "failed"}>
            <Card title="Cannot load your ledgers">
              <p data-testid="ledgers-failed">
                {(data.ledgersStatus() as { message?: string }).message}
              </p>
            </Card>
          </Match>
          <Match when={screen() === "loading"}>
            <Waiting label="Loading your ledgers…" />
          </Match>
          <Match when={screen() === "onboarding"}>
            <Onboarding />
          </Match>
          <Match when={screen() === "wallet"}>
            <Shell />
          </Match>
        </Switch>
      </Router>
    </NodeDataContext>
  );
}

function Unreachable(props: { message: string }): JSX.Element {
  return (
    <Card title="Cannot reach your node">
      <p>
        The wallet could not ask your Dassie node for its state:{" "}
        {props.message}.
      </p>
      <div class="actions">
        <button
          type="button"
          class="primary"
          onClick={() => location.reload()}
        >
          Try again
        </button>
      </div>
    </Card>
  );
}

function SetupInfo(): JSX.Element {
  return (
    <Card title="Dassie Wallet" testid="setup-info">
      <p>
        This Dassie node is ready for setup. Please visit its setup URL, which
        this command prints on the server:
      </p>
      <pre><code>sudo dassie setup-url</code></pre>
    </Card>
  );
}
