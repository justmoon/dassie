/**
 * The setup wizard (spec/areas/wallet.md#key-derivation), shown at
 * `/setup/<token>` while the node has no key: create or recover the passphrase,
 * then hand the node its key. The steps are a local state machine; nothing
 * about them is kept in the URL, so a reload starts over, and the passphrase is
 * never stored.
 */

import { isPhraseError, loadIdentity } from "@dassie/wasm/web";
import type { JSX } from "@solidjs/web";
import { createSignal, Match, Show, Switch } from "solid-js";
import { submitSetup } from "../auth.ts";
import {
  IDENTITY_FAILED,
  NODE_CLOCK_UNKNOWN,
  phraseNotMade,
  realmName,
  setupFailure,
} from "../messages.ts";
import { namesWord, nodeClock, type Realm } from "../phrase.ts";
import { Card, Spinner } from "../ui.tsx";
import { PhraseEntry, PhraseGrid } from "./phrase.tsx";

/** A phrase the wallet made, as shown to the owner. */
interface Made {
  /** The canonical phrase. */
  readonly text: string;
  readonly words: readonly string[];
  /** The day it was made, `2026-02-19`. */
  readonly date: string;
}

type Step =
  | { readonly name: "intro" }
  | { readonly name: "generate"; readonly made: Made }
  | { readonly name: "verify"; readonly made: Made }
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

/**
 * The setup wizard, for the setup token in the URL, setting the node up in
 * `realm`, the one it reports.
 */
export function Setup(props: { token: string; realm: Realm }): JSX.Element {
  const [step, setStep] = createSignal<Step>({ name: "intro" });
  // The key derivation module is needed from the first button on; loading
  // it now hides the delay. A failure surfaces when it is used.
  loadIdentity().catch(() => {});
  const go: Go = (next) => setStep(next);
  return (
    <Switch>
      <Match when={stepIf(step(), "intro")}>
        <Intro realm={props.realm} go={go} />
      </Match>
      <Match when={stepIf(step(), "generate")}>
        {(current) => <Generate made={current().made} go={go} />}
      </Match>
      <Match when={stepIf(step(), "verify")}>
        {(current) => <Verify made={current().made} go={go} />}
      </Match>
      <Match when={stepIf(step(), "recover")}>
        <Recover realm={props.realm} go={go} />
      </Match>
      <Match when={stepIf(step(), "opening")}>
        {(current) => (
          <Opening
            token={props.token}
            realm={props.realm}
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

/**
 * A new phrase of `realm`, made now by this browser's clock, once that
 * clock is within a day of the node's (spec/decisions/phrase-format.md);
 * or why none was made.
 */
async function makePhrase(realm: Realm): Promise<Made | string> {
  let identity;
  try {
    identity = await loadIdentity();
  } catch {
    return IDENTITY_FAILED;
  }
  const nodeNowMs = await nodeClock();
  if (nodeNowMs === undefined) return NODE_CLOCK_UNKNOWN;
  let made;
  try {
    made = identity.generatePhrase(realm, Date.now(), nodeNowMs);
  } catch (error) {
    if (isPhraseError(error)) return phraseNotMade(error);
    throw error;
  }
  try {
    return { text: made.text, words: made.words, date: made.createdDate };
  } finally {
    made.free();
  }
}

function Intro(props: { realm: Realm; go: Go }): JSX.Element {
  const [error, setError] = createSignal<string>();
  const [pending, setPending] = createSignal(false);
  const generate = async () => {
    if (pending()) return;
    setError(undefined);
    setPending(true);
    const made = await makePhrase(props.realm).catch(() => IDENTITY_FAILED);
    setPending(false);
    if (typeof made === "string") setError(made);
    else props.go({ name: "generate", made });
  };
  return (
    <Card title="Dassie Wallet">
      <p>
        Welcome to Dassie Wallet. The first step is to create or recover a
        wallet.
      </p>
      <Show when={props.realm === "live"}>
        <p data-testid="setup-realm">
          This node runs on {realmName(props.realm)}, with real money.
        </p>
      </Show>
      <ErrorText message={error()} />
      <div class="actions">
        <button
          type="button"
          class="primary"
          data-testid="generate-wallet"
          disabled={pending()}
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

function Generate(props: { made: Made; go: Go }): JSX.Element {
  return (
    <Card title="Generate Wallet">
      <p>
        These 20 words are your wallet. Write them down in order and keep them
        somewhere safe and private. Anyone who knows them controls your node and
        its money, and if you lose them, nobody can recover them for you.
      </p>
      <p class="hint">
        A metal plate made for 24 words holds them: it needs only the first four
        letters of each word, shown in bold.
      </p>
      <PhraseGrid words={props.made.words} testid="mnemonic" />
      <p class="phrase-date" data-testid="phrase-date">
        Generated {props.made.date}
      </p>
      <div class="actions">
        <button type="button" onClick={() => props.go({ name: "intro" })}>
          Back
        </button>
        <button
          type="button"
          class="primary"
          data-testid="backed-up"
          onClick={() => props.go({ name: "verify", made: props.made })}
        >
          I have written it down
        </button>
      </div>
    </Card>
  );
}

/** A uniformly random index below `count`, from the platform's CSPRNG. */
function randomIndex(count: number): number {
  const [value] = crypto.getRandomValues(new Uint32Array(1));
  // 2^32 is not a multiple of 20, but the bias is 16 in 2^32: none worth
  // a rejection loop.
  return value % count;
}

function Verify(props: { made: Made; go: Go }): JSX.Element {
  const index = randomIndex(props.made.words.length);
  const word = props.made.words[index];
  const [answer, setAnswer] = createSignal("");
  const [correct, setCorrect] = createSignal(false);
  const check = (typed: string) => {
    setAnswer(typed);
    // A word is matched by its first four letters, as a reading matches it,
    // so an owner who wrote only those can answer with them.
    loadIdentity().then(
      (identity) => setCorrect(namesWord(identity, typed, word)),
      () => setCorrect(false),
    );
  };
  const submit = (event: SubmitEvent) => {
    event.preventDefault();
    if (!correct()) return;
    props.go({
      name: "opening",
      phrase: props.made.text,
      back: { name: "verify", made: props.made },
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
          onInput={(event) => check(event.currentTarget.value)}
        />
        <div class="actions">
          <button
            type="button"
            onClick={() => props.go({ name: "generate", made: props.made })}
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

function Recover(props: { realm: Realm; go: Go }): JSX.Element {
  return (
    <Card title="Recover Wallet">
      <PhraseEntry
        realm={props.realm}
        labels={{
          id: "recover-phrase",
          input: "recover-input",
          submit: "recover-submit",
          error: "setup-error",
          action: "Open Wallet",
        }}
        onPhrase={(phrase) => {
          props.go({ name: "opening", phrase, back: { name: "recover" } });
          return Promise.resolve(undefined);
        }}
        actions={
          <button
            type="button"
            onClick={() => props.go({ name: "intro" })}
          >
            Back
          </button>
        }
      />
    </Card>
  );
}

function Opening(props: {
  token: string;
  realm: Realm;
  phrase: string;
  back: Step;
  go: Go;
}): JSX.Element {
  const [error, setError] = createSignal<string>();
  // The step is entered once per attempt, so the request goes out once,
  // when the step is created; "Back" leaves it, and a new attempt is a new
  // step.
  const back = props.back;
  submitSetup(props.token, props.phrase, props.realm).then(
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
