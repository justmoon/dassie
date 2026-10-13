/**
 * The setup wizard (spec 6 §1.6.1), shown at `/setup/<token>` while the node
 * has no key: create or recover the passphrase, then hand the node its key.
 * The steps are a local state machine; nothing about them is kept in the
 * URL, so a reload starts over, and the passphrase is never stored.
 */

import { type Identity, loadIdentity } from "@dassie/identity-wasm/web";
import type { JSX } from "@solidjs/web";
import { createSignal, Match, Show, Switch } from "solid-js";
import { checkPassphrase, submitSetup } from "./auth.ts";
import {
  IDENTITY_FAILED,
  INVALID_PASSPHRASE,
  setupFailure,
} from "./messages.ts";
import { Card, Spinner } from "./ui.tsx";

type Step =
  | { readonly name: "intro" }
  | { readonly name: "generate"; readonly mnemonic: string }
  | { readonly name: "verify"; readonly mnemonic: string }
  | { readonly name: "recover" }
  | {
    readonly name: "opening";
    readonly phrase: string;
    /** Where "Back" leads if the node refuses. */
    readonly back: Step;
  };

type Go = (step: Step) => void;

/** `step`, if it is the named one, typed as such. */
function stepIf<N extends Step["name"]>(
  step: Step,
  name: N,
): Extract<Step, { name: N }> | undefined {
  return step.name === name ? step as Extract<Step, { name: N }> : undefined;
}

/** The setup wizard, for the setup token in the URL. */
export function Setup(props: { token: string }): JSX.Element {
  const [step, setStep] = createSignal<Step>({ name: "intro" });
  // The key derivation module is needed from the first button on; loading
  // it now hides the delay. A failure surfaces when it is used.
  loadIdentity().catch(() => {});
  const go: Go = (next) => setStep(next);
  return (
    <Switch>
      <Match when={stepIf(step(), "intro")}>
        <Intro go={go} />
      </Match>
      <Match when={stepIf(step(), "generate")}>
        {(current) => <Generate mnemonic={current().mnemonic} go={go} />}
      </Match>
      <Match when={stepIf(step(), "verify")}>
        {(current) => <Verify mnemonic={current().mnemonic} go={go} />}
      </Match>
      <Match when={stepIf(step(), "recover")}>
        <Recover go={go} />
      </Match>
      <Match when={stepIf(step(), "opening")}>
        {(current) => (
          <Opening
            token={props.token}
            phrase={current().phrase}
            back={current().back}
            go={go}
          />
        )}
      </Match>
    </Switch>
  );
}

function ErrorText(props: { message: string | undefined }): JSX.Element {
  return (
    <Show when={props.message}>
      <p class="error" role="alert" data-testid="setup-error">
        {props.message}
      </p>
    </Show>
  );
}

function Intro(props: { go: Go }): JSX.Element {
  const [error, setError] = createSignal<string>();
  const generate = async () => {
    try {
      const identity = await loadIdentity();
      props.go({ name: "generate", mnemonic: identity.generateMnemonic() });
    } catch {
      setError(IDENTITY_FAILED);
    }
  };
  return (
    <Card title="Dassie Wallet">
      <p>
        Welcome to Dassie Wallet. The first step is to create or recover a
        wallet.
      </p>
      <ErrorText message={error()} />
      <div class="actions">
        <button
          type="button"
          class="primary"
          data-testid="generate-wallet"
          onClick={generate}
        >
          Generate a new wallet
        </button>
        <button
          type="button"
          class="link"
          data-testid="recover-wallet"
          onClick={() => props.go({ name: "recover" })}
        >
          Recover an existing wallet
        </button>
      </div>
    </Card>
  );
}

function Generate(props: { mnemonic: string; go: Go }): JSX.Element {
  return (
    <Card title="Generate Wallet">
      <p>
        This passphrase is your wallet. Write it down and keep it somewhere safe
        and private. Anyone who knows it controls your node and its money, and
        if you lose it, nobody can recover it for you.
      </p>
      <p class="mnemonic" data-testid="mnemonic">{props.mnemonic}</p>
      <div class="actions">
        <button type="button" onClick={() => props.go({ name: "intro" })}>
          Back
        </button>
        <button
          type="button"
          class="primary"
          data-testid="backed-up"
          onClick={() => props.go({ name: "verify", mnemonic: props.mnemonic })}
        >
          Ok, I have backed up my passphrase
        </button>
      </div>
    </Card>
  );
}

/** A uniformly random index below `count`, from the platform's CSPRNG. */
function randomIndex(count: number): number {
  const [value] = crypto.getRandomValues(new Uint32Array(1));
  // 2^32 is not a multiple of 12, but the bias is 4 in 2^32: none worth
  // a rejection loop.
  return value % count;
}

function Verify(props: { mnemonic: string; go: Go }): JSX.Element {
  const words = props.mnemonic.split(" ");
  const index = randomIndex(words.length);
  const [answer, setAnswer] = createSignal("");
  const correct = () => answer().trim().toLowerCase() === words[index];
  const submit = (event: SubmitEvent) => {
    event.preventDefault();
    if (!correct()) return;
    props.go({
      name: "opening",
      phrase: props.mnemonic,
      back: { name: "verify", mnemonic: props.mnemonic },
    });
  };
  return (
    <Card title="Verify Passphrase">
      <form onSubmit={submit}>
        <label for="verify-word">
          Please enter word #<span data-testid="verify-word-number">
            {index + 1}
          </span>
        </label>
        <input
          id="verify-word"
          type="text"
          data-testid="verify-input"
          autocomplete="off"
          autocapitalize="none"
          spellcheck={false}
          value={answer()}
          onInput={(event) => setAnswer(event.currentTarget.value)}
        />
        <div class="actions">
          <button
            type="button"
            onClick={() =>
              props.go({ name: "generate", mnemonic: props.mnemonic })}
          >
            Back
          </button>
          <button
            type="submit"
            class="primary"
            data-testid="verify-submit"
            disabled={!correct()}
          >
            Verify
          </button>
        </div>
      </form>
    </Card>
  );
}

function Recover(props: { go: Go }): JSX.Element {
  const [phrase, setPhrase] = createSignal("");
  const [error, setError] = createSignal<string>();
  const submit = async (event: SubmitEvent) => {
    event.preventDefault();
    const entered = phrase();
    let identity: Identity;
    try {
      identity = await loadIdentity();
    } catch {
      setError(IDENTITY_FAILED);
      return;
    }
    const normalized = checkPassphrase(identity, entered);
    if (normalized === undefined) {
      setError(INVALID_PASSPHRASE);
      return;
    }
    props.go({
      name: "opening",
      phrase: normalized,
      back: { name: "recover" },
    });
  };
  return (
    <Card title="Recover Wallet">
      <form onSubmit={submit}>
        <label for="recover-phrase">Passphrase</label>
        <textarea
          id="recover-phrase"
          data-testid="recover-input"
          rows={3}
          autocomplete="off"
          autocapitalize="none"
          spellcheck={false}
          placeholder="correct horse staple …"
          value={phrase()}
          onInput={(event) => {
            setPhrase(event.currentTarget.value);
            setError(undefined);
          }}
        />
        <ErrorText message={error()} />
        <div class="actions">
          <button type="button" onClick={() => props.go({ name: "intro" })}>
            Back
          </button>
          <button type="submit" class="primary" data-testid="recover-submit">
            Open Wallet
          </button>
        </div>
      </form>
    </Card>
  );
}

function Opening(props: {
  token: string;
  phrase: string;
  back: Step;
  go: Go;
}): JSX.Element {
  const [error, setError] = createSignal<string>();
  // The step is entered once per attempt, so the request goes out once,
  // when the step is created; "Back" leaves it, and a new attempt is a new
  // step.
  const back = props.back;
  submitSetup(props.token, props.phrase).then(
    (outcome) => {
      if (outcome.ok) {
        // The setup URL's token is spent; the wallet lives at the root.
        history.replaceState({}, "", "/");
        location.reload();
        return;
      }
      setError(setupFailure(outcome));
    },
    () => setError(IDENTITY_FAILED),
  );
  return (
    <Card title="Opening Wallet">
      <Show
        when={error()}
        fallback={<Spinner label="Setting up your node…" />}
      >
        <ErrorText message={error()} />
        <div class="actions">
          <button type="button" onClick={() => props.go(back)}>
            Back
          </button>
        </div>
      </Show>
    </Card>
  );
}
