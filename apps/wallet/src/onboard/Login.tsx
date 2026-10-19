/**
 * The login page (spec/areas/wallet.md#the-gate), shown whenever the node is set
 * up and this browser has no session. Unlike the original, it checks the
 * passphrase before deriving from it and says why a login failed.
 */

import { type Identity, loadIdentity } from "@dassie/identity-wasm/web";
import type { JSX } from "@solidjs/web";
import { createSignal, Show } from "solid-js";
import { checkPassphrase, submitLogin } from "../auth.ts";
import {
  IDENTITY_FAILED,
  INVALID_PASSPHRASE,
  loginFailure,
} from "../messages.ts";
import { Card } from "../ui.tsx";

/** Asks for the passphrase and signs the owner in with its proof. */
export function Login(): JSX.Element {
  const [phrase, setPhrase] = createSignal("");
  const [error, setError] = createSignal<string>();
  const [pending, setPending] = createSignal(false);
  // Loading the key derivation module while the passphrase is typed hides
  // its delay; a failure surfaces on submit.
  loadIdentity().catch(() => {});

  /** Signs in; resolves to what went wrong, or never, as the page reloads. */
  const attempt = async (entered: string): Promise<string | undefined> => {
    let identity: Identity;
    try {
      identity = await loadIdentity();
    } catch {
      return IDENTITY_FAILED;
    }
    const normalized = checkPassphrase(identity, entered);
    if (normalized === undefined) return INVALID_PASSPHRASE;
    const outcome = await submitLogin(normalized);
    if (!outcome.ok) return loginFailure(outcome);
    location.reload();
    return undefined;
  };

  const submit = async (event: SubmitEvent) => {
    event.preventDefault();
    if (pending()) return;
    setError(undefined);
    setPending(true);
    const failure = await attempt(phrase());
    // On success the page is reloading: the button stays pending until then.
    if (failure === undefined) return;
    setError(failure);
    setPending(false);
  };

  return (
    <Card
      title="Reconnect to your Dassie instance"
      footer={
        <p class="footer">
          <a href="https://dassie.land" rel="noopener noreferrer">
            What is Dassie?
          </a>
        </p>
      }
    >
      <p class="subtitle">Enter your wallet passphrase</p>
      <form onSubmit={submit}>
        <label for="login-phrase">Passphrase</label>
        <input
          id="login-phrase"
          type="text"
          data-testid="login-input"
          autocomplete="off"
          autocapitalize="none"
          spellcheck={false}
          placeholder="correct horse staple …"
          value={phrase()}
          onInput={(event) => setPhrase(event.currentTarget.value)}
        />
        <Show when={error()}>
          <p class="error" role="alert" data-testid="login-error">{error()}</p>
        </Show>
        <div class="actions">
          <button
            type="submit"
            class="primary"
            data-testid="login-submit"
            disabled={pending()}
          >
            {pending() ? "Reconnecting…" : "Reconnect"}
          </button>
        </div>
      </form>
    </Card>
  );
}
