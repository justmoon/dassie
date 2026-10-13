/**
 * The gate (spec 6 §1.5): which screen the wallet shows follows from the
 * node's state, as `getBasicState` reports it to this connection.
 *
 * - uninitialized: the setup wizard at `/setup/<token>`, otherwise a note
 *   on where to find that URL;
 * - anonymous: the login page, whatever the URL;
 * - authenticated: the wallet.
 */

import { RpcClient } from "@dassie/rpc";
import { type GetBasicStateResponse, NodeState } from "@dassie/rpc/proto";
import type { JSX } from "@solidjs/web";
import { createSignal, Match, Switch } from "solid-js";
import { Home } from "./Home.tsx";
import { Login } from "./Login.tsx";
import { Setup } from "./Setup.tsx";
import { Card } from "./ui.tsx";

/** The token of a setup URL, `/setup/<64 hex digits>`. */
const SETUP_PATH = /^\/setup\/([0-9a-fA-F]{64})\/?$/;

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

  return (
    <Switch>
      <Match when={failure()}>
        {(message) => (
          <Card title="Cannot reach your node">
            <p>
              The wallet could not ask your Dassie node for its state:{" "}
              {message()}.
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
        )}
      </Match>
      <Match when={basic()}>
        {(state) => <Gate basic={state()} />}
      </Match>
    </Switch>
  );
}

function Gate(props: { basic: GetBasicStateResponse }): JSX.Element {
  const token = SETUP_PATH.exec(location.pathname)?.[1]?.toLowerCase();
  return (
    <Switch
      fallback={
        <Card title="Unexpected node state">
          <p>
            Your Dassie node reported a state this wallet does not know. Is the
            wallet older than the node?
          </p>
        </Card>
      }
    >
      <Match when={props.basic.state === NodeState.UNINITIALIZED}>
        {token === undefined ? <SetupInfo /> : <Setup token={token} />}
      </Match>
      <Match when={props.basic.state === NodeState.ANONYMOUS}>
        <Login />
      </Match>
      <Match when={props.basic.state === NodeState.AUTHENTICATED}>
        <Home nodeId={props.basic.nodeId} hostname={props.basic.hostname} />
      </Match>
    </Switch>
  );
}

function SetupInfo(): JSX.Element {
  return (
    <Card title="Dassie Wallet" testid="setup-info">
      <p>
        This Dassie node is ready for setup. Please visit the setup URL provided
        in your server logs or the output of the <code>dassie init</code>{" "}
        command.
      </p>
    </Card>
  );
}
