/**
 * The owner's phrase in the wallet (spec/decisions/phrase-format.md): the
 * realm the wallet makes and reads phrases in, the node's clock that a new
 * phrase is checked against, and what the owner typed, read into a phrase,
 * a correction for them to confirm, or a refusal in their terms.
 */

import type { Identity, PhraseReading } from "@dassie/wasm/web";
import { isPhraseError } from "@dassie/wasm/web";
import { phraseRefusal } from "./messages.ts";

/** A realm, as the identity module names it. */
export type Realm = "test" | "live";

/** Words in a phrase. */
export const PHRASE_WORDS = 20;

/** Letters of a word that identify it, and all a metal plate keeps. */
export const KEY_LETTERS = 4;

/**
 * The realm of a node, from the first segment of its ILP address as
 * `GetBasicState` reports it, or undefined for one the wallet does not
 * know. Before setup it is the realm the node sets up in.
 */
export function realmOf(ilpAllocationScheme: string): Realm | undefined {
  switch (ilpAllocationScheme) {
    case "test":
      return "test";
    case "g":
      return "live";
    default:
      return undefined;
  }
}

/**
 * A word split into the letters that identify it and the rest, so a page
 * can show which letters a plate needs.
 */
export function keyLetters(word: string): [key: string, rest: string] {
  return [word.slice(0, KEY_LETTERS), word.slice(KEY_LETTERS)];
}

/**
 * The time a `Date` header gives, in milliseconds since the Unix epoch,
 * or undefined if there is none or it is not a date.
 */
export function clockOfDate(header: string | null): number | undefined {
  if (header === null) return undefined;
  const ms = Date.parse(header);
  return Number.isFinite(ms) ? ms : undefined;
}

/**
 * The node's clock, from the `Date` header of a response from its own
 * origin, which the node's HTTP server sets from its host's clock; undefined
 * if it could not be read. A new phrase is checked against it, since a
 * phrase made by a clock that runs ahead is refused until its date.
 */
export async function nodeClock(): Promise<number | undefined> {
  try {
    const response = await fetch("/", {
      method: "HEAD",
      cache: "no-store",
      credentials: "same-origin",
    });
    return clockOfDate(response.headers.get("Date"));
  } catch {
    return undefined;
  }
}

/**
 * Whether `typed` names `word`, as a reading matches a word: by its first
 * four letters, in any case.
 */
export function namesWord(
  identity: Pick<Identity, "phraseWords">,
  typed: string,
  word: string,
): boolean {
  try {
    const read = identity.phraseWords(typed);
    return read.length === 1 && read[0] === word;
  } catch {
    return false;
  }
}

/**
 * What each word the owner typed reads as: the word of the list it names,
 * or undefined for one not on the list. Undefined while the text is longer
 * than any phrase could be.
 */
export function typedWords(
  identity: Pick<Identity, "phraseWords">,
  text: string,
): (string | undefined)[] | undefined {
  try {
    return identity.phraseWords(text);
  } catch (error) {
    if (isPhraseError(error)) return undefined;
    throw error;
  }
}

/**
 * A phrase the reading had to correct: the words it reads as, with the
 * positions that changed, for the owner to compare with their copy. Its
 * phrase is taken only through {@link Correction.confirm}.
 */
export interface Correction {
  /** The 20 words, corrected. */
  readonly words: readonly string[];
  /** The zero-based positions of the words that changed. */
  readonly changed: ReadonlySet<number>;
  /** The day the phrase was made, `2026-02-19`. */
  readonly date: string;
  /** The phrase in canonical form, once the owner has confirmed it. */
  confirm(): string;
  /** Drops the correction unconfirmed, wiping it from the module. */
  dismiss(): void;
}

/** What reading what the owner typed found. */
export type Entry =
  /** The phrase as written, in canonical form. */
  | { readonly kind: "phrase"; readonly text: string }
  /** A correction, to show and confirm before anything is derived. */
  | { readonly kind: "corrected"; readonly correction: Correction }
  /** Refused, with why, in the owner's terms. */
  | { readonly kind: "refused"; readonly message: string };

/**
 * Reads what the owner typed as a phrase of `realm` by a clock that reads
 * `nowMs`, with the zero-based positions in `unreadable` marked as words
 * they cannot read. A refusal of kind, realm or time comes before any
 * correction (spec/decisions/phrase-format.md#decision).
 */
export function readEntry(
  identity: Pick<Identity, "readPhrase">,
  text: string,
  unreadable: Iterable<number>,
  realm: Realm,
  nowMs: number,
): Entry {
  const marked = Uint32Array.from(
    [...unreadable].filter((position) => position < PHRASE_WORDS),
  );
  let reading: PhraseReading;
  try {
    reading = identity.readPhrase(text, realm, nowMs, marked);
  } catch (error) {
    if (isPhraseError(error)) {
      return { kind: "refused", message: phraseRefusal(error, realm) };
    }
    throw error;
  }
  if (!reading.corrected) {
    return { kind: "phrase", text: spend(reading.exact()) };
  }
  let open = true;
  const correction: Correction = {
    words: reading.words,
    changed: new Set(reading.changed),
    date: reading.createdDate,
    confirm() {
      if (!open) throw new Error("the correction is spent");
      open = false;
      return spend(reading.confirm());
    },
    dismiss() {
      if (!open) return;
      open = false;
      reading.free();
    },
  };
  return { kind: "corrected", correction };
}

/** A phrase's canonical text, freeing the phrase. */
function spend(phrase: { readonly text: string; free(): void }): string {
  try {
    return phrase.text;
  } finally {
    phrase.free();
  }
}
