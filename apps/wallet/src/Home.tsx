/**
 * What the owner sees once signed in. M2 only proves that setup and login
 * work end to end; M5 replaces this page with the full wallet (account,
 * send, receive, ledgers, network, settings, debug).
 */

import type { JSX } from "@solidjs/web";
import { createSignal, Show } from "solid-js";
import { submitLogout } from "./auth.ts";
import { logoutFailure } from "./messages.ts";
import { Card } from "./ui.tsx";

/** The node's identity, and a way to log out. */
export function Home(props: { nodeId: string; hostname: string }): JSX.Element {
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal<string>();
  const logout = async () => {
    setPending(true);
    setError(undefined);
    const outcome = await submitLogout();
    if (outcome.ok) {
      // Without the cookie the gate shows the login page.
      location.reload();
      return;
    }
    setError(logoutFailure(outcome));
    setPending(false);
  };
  return (
    <Card title="Setup complete" testid="setup-complete">
      <p>Your Dassie node is set up, and you are signed in as its owner.</p>
      <dl class="facts">
        <dt>Node ID</dt>
        <dd>
          <code data-testid="node-id">{props.nodeId}</code>
        </dd>
        <Show when={props.hostname}>
          <dt>Hostname</dt>
          <dd>
            <code data-testid="hostname">{props.hostname}</code>
          </dd>
        </Show>
      </dl>
      <Show when={error()}>
        <p class="error" role="alert" data-testid="logout-error">{error()}</p>
      </Show>
      <div class="actions">
        <button
          type="button"
          data-testid="logout"
          disabled={pending()}
          onClick={logout}
        >
          Log out
        </button>
      </div>
    </Card>
  );
}
