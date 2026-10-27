/**
 * The owner's phrase on screen (spec/decisions/phrase-format.md): its 20
 * words numbered, in two columns of ten as a metal plate lays them out,
 * with the four letters a plate keeps in bold; and the field the owner
 * enters it in, which shows what each word reads as, lets them mark a word
 * they cannot read, and shows a correction word by word for them to
 * confirm before the phrase is used.
 */

import { type Identity, loadIdentity } from "@dassie/wasm/web";
import type { JSX } from "@solidjs/web";
import { createSignal, For, onCleanup, Show } from "solid-js";
import { IDENTITY_FAILED } from "../messages.ts";
import {
  type Correction,
  keyLetters,
  PHRASE_WORDS,
  readEntry,
  type Realm,
  typedWords,
} from "../phrase.ts";

/**
 * A phrase's words, numbered from one, down the first column and then the
 * second. The words at `changed` are marked as corrected. The list's text
 * is the words with single spaces between them, numbers aside.
 */
export function PhraseGrid(props: {
  words: readonly string[];
  changed?: ReadonlySet<number>;
  testid: string;
}): JSX.Element {
  return (
    <ol class="phrase" role="list" data-testid={props.testid}>
      <For each={props.words}>
        {(word, index) => {
          const [key, rest] = keyLetters(word);
          const changed = () => props.changed?.has(index()) ?? false;
          return (
            <li
              class={changed() ? "changed" : undefined}
              data-testid="phrase-word"
            >
              <b>{key}</b>
              {rest}
              <Show when={changed()}>
                <span class="sr-only">, corrected</span>
              </Show>
              {" "}
            </li>
          );
        }}
      </For>
    </ol>
  );
}

/** The ways a phrase entry is labeled, for the page it is on. */
export interface EntryLabels {
  /** The field's `id`, which its label names. */
  readonly id: string;
  /** The `data-testid` of the field, its submit button and its error. */
  readonly input: string;
  readonly submit: string;
  readonly error: string;
  /** What the submit button says, and says while the phrase is used. */
  readonly action: string;
  readonly pending?: string;
}

/**
 * The field the owner enters a phrase in, for `realm`. Once the phrase
 * reads, as written or with a correction the owner confirmed, it goes to
 * `onPhrase` in canonical form, which resolves to why using it failed, or
 * to undefined once the page moves on.
 */
export function PhraseEntry(props: {
  realm: Realm;
  labels: EntryLabels;
  onPhrase: (phrase: string) => Promise<string | undefined>;
  /** More actions beside submit, such as Back. */
  actions?: JSX.Element;
}): JSX.Element {
  const [identity, setIdentity] = createSignal<Identity>();
  const [text, setText] = createSignal("");
  const [unreadable, setUnreadable] = createSignal<readonly number[]>([]);
  const [error, setError] = createSignal<string>();
  const [correction, setCorrection] = createSignal<Correction>();
  const [pending, setPending] = createSignal(false);
  // Loading the identity module while the phrase is typed hides its
  // delay; until it is there, the words are not shown as read. A failure
  // surfaces on submit.
  loadIdentity().then((loaded) => setIdentity(() => loaded), () => {});
  // A correction left unconfirmed is wiped from the module.
  onCleanup(() => correction()?.dismiss());

  const read = () => {
    const loaded = identity();
    return loaded && text().trim() !== ""
      ? typedWords(loaded, text())
      : undefined;
  };
  const toggle = (position: number) =>
    setUnreadable((marked) =>
      marked.includes(position)
        ? marked.filter((other) => other !== position)
        : [...marked, position]
    );

  const use = async (phrase: string) => {
    setPending(true);
    const failure = await props.onPhrase(phrase);
    // Undefined: the page has moved on, and the button stays pending.
    if (failure === undefined) return;
    setError(failure);
    setPending(false);
  };

  const submit = async (event: SubmitEvent) => {
    event.preventDefault();
    if (pending()) return;
    setError(undefined);
    let loaded: Identity;
    try {
      loaded = await loadIdentity();
    } catch {
      setError(IDENTITY_FAILED);
      return;
    }
    const entry = readEntry(
      loaded,
      text(),
      unreadable(),
      props.realm,
      Date.now(),
    );
    switch (entry.kind) {
      case "refused":
        setError(entry.message);
        return;
      case "corrected":
        setCorrection(() => entry.correction);
        return;
      case "phrase":
        await use(entry.text);
        return;
    }
  };

  const confirm = async () => {
    const shown = correction();
    if (!shown) return;
    const phrase = shown.confirm();
    setCorrection(undefined);
    await use(phrase);
  };

  const dismiss = () => {
    correction()?.dismiss();
    setCorrection(undefined);
  };

  return (
    <Show
      when={correction()}
      fallback={
        <form onSubmit={submit}>
          <label for={props.labels.id}>Your 20 words, in order</label>
          <p class="hint" id={`${props.labels.id}-hint`}>
            The first four letters of each word are enough. Type <kbd>?</kbd>
            {" "}
            for a word you cannot read.
          </p>
          <textarea
            id={props.labels.id}
            class="phrase-input"
            data-testid={props.labels.input}
            aria-describedby={`${props.labels.id}-hint`}
            rows={4}
            autocomplete="off"
            autocapitalize="none"
            autocorrect="off"
            spellcheck={false}
            placeholder="aban abil able abou …"
            value={text()}
            onInput={(event) => {
              setText(event.currentTarget.value);
              setError(undefined);
            }}
            onKeyDown={(event: KeyboardEvent) => {
              // Enter submits, as in a one-line field; the words need no
              // line breaks.
              if (event.key === "Enter" && !event.shiftKey) {
                event.preventDefault();
                (event.currentTarget as HTMLTextAreaElement).form
                  ?.requestSubmit();
              }
            }}
          />
          <Show when={read()}>
            {(words) => (
              <TypedWords
                words={words()}
                unreadable={unreadable()}
                toggle={toggle}
              />
            )}
          </Show>
          <Show when={error()}>
            <p class="error" role="alert" data-testid={props.labels.error}>
              {error()}
            </p>
          </Show>
          <div class="actions">
            {props.actions}
            <button
              type="submit"
              class="primary"
              data-testid={props.labels.submit}
              disabled={pending()}
            >
              {pending() && props.labels.pending
                ? props.labels.pending
                : props.labels.action}
            </button>
          </div>
        </form>
      }
    >
      {(shown) => (
        <ConfirmCorrection
          correction={shown()}
          confirm={confirm}
          dismiss={dismiss}
        />
      )}
    </Show>
  );
}

/**
 * What the owner typed, word by word, as the wallet reads it: each word
 * numbered and in full, a word not on the list flagged, and the owner's
 * marks on words they cannot read.
 */
function TypedWords(props: {
  words: readonly (string | undefined)[];
  unreadable: readonly number[];
  toggle: (position: number) => void;
}): JSX.Element {
  const unknown = () => props.words.filter((word) => word === undefined).length;
  const marked = () =>
    props.unreadable.filter((position) =>
      position < props.words.length && props.words[position] !== undefined
    ).length;
  return (
    <div class="typed" data-testid="typed-words">
      <p class="hint" data-testid="typed-count">
        {props.words.length} of {PHRASE_WORDS} words
        <Show when={unknown() > 0}>, {unknown()} not on the list</Show>
        <Show when={marked() > 0}>, {marked()} marked unreadable</Show>.
        <Show when={props.words.length > 0}>
          {" "}Tap a word you are unsure of to mark it unreadable.
        </Show>
      </p>
      <ol class="phrase typed-words" role="list">
        <For each={props.words} keyed={false}>
          {(word, index) => (
            <li>
              <Show
                when={word()}
                fallback={
                  <span class="unknown" data-testid="typed-unknown">
                    not a word
                  </span>
                }
              >
                {(known) => (
                  <button
                    type="button"
                    class="word-toggle"
                    aria-pressed={props.unreadable.includes(index)
                      ? "true"
                      : "false"}
                    data-testid="typed-word"
                    onClick={() => props.toggle(index)}
                  >
                    {known()}
                  </button>
                )}
              </Show>
            </li>
          )}
        </For>
      </ol>
    </div>
  );
}

/**
 * A correction, shown word by word with the words it changed marked, used
 * only once the owner confirms it.
 */
function ConfirmCorrection(props: {
  correction: Correction;
  confirm: () => void;
  dismiss: () => void;
}): JSX.Element {
  const count = props.correction.changed.size;
  return (
    <div data-testid="phrase-correction">
      <p>
        {count === 1 ? "One word was" : `${count} words were`}{" "}
        wrong or unreadable, and the wallet restored{" "}
        {count === 1 ? "it" : "them"}. Compare the marked{" "}
        {count === 1 ? "word" : "words"}{" "}
        with your copy before you go on, and fix your copy if it is wrong.
      </p>
      <PhraseGrid
        words={props.correction.words}
        changed={props.correction.changed}
        testid="corrected-phrase"
      />
      <p class="hint">
        This phrase was made on {props.correction.date}.
      </p>
      <div class="actions">
        <button
          type="button"
          data-testid="correction-back"
          onClick={() => props.dismiss()}
        >
          Back
        </button>
        <button
          type="button"
          class="primary"
          data-testid="correction-confirm"
          onClick={() => props.confirm()}
        >
          Use these words
        </button>
      </div>
    </div>
  );
}
