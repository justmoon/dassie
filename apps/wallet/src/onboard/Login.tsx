/**
 * The login page (spec/areas/wallet.md#the-gate), shown whenever the node is set
 * up and this browser has no session. Unlike the original, it checks the
 * passphrase before deriving from it and says why a login failed.
 */

import { loadIdentity } from "@dassie/wasm/web";
import type { JSX } from "@solidjs/web";
import { submitLogin } from "../auth.ts";
import { IDENTITY_FAILED, loginFailure } from "../messages.ts";
import type { Realm } from "../phrase.ts";
import { Card } from "../ui.tsx";
import { PhraseEntry } from "./phrase.tsx";

/**
 * Asks for the passphrase, a phrase of `realm`, the node's, and signs the
 * owner in with its proof.
 */
export function Login(props: { realm: Realm }): JSX.Element {
  // Loading the key derivation module while the passphrase is typed hides
  // its delay; a failure surfaces on submit.
  loadIdentity().catch(() => {});

  /** Signs in; resolves to what went wrong, or never, as the page reloads. */
  const signIn = async (phrase: string): Promise<string | undefined> => {
    let outcome;
    try {
      outcome = await submitLogin(phrase, props.realm);
    } catch {
      return IDENTITY_FAILED;
    }
    if (!outcome.ok) return loginFailure(outcome);
    location.reload();
    return undefined;
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
      <PhraseEntry
        realm={props.realm}
        labels={{
          id: "login-phrase",
          input: "login-input",
          submit: "login-submit",
          error: "login-error",
          action: "Reconnect",
          pending: "Reconnecting…",
        }}
        onPhrase={signIn}
      />
    </Card>
  );
}
