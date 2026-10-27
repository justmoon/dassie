/**
 * Why a phrase was refused (spec/decisions/phrase-format.md): the identity
 * module throws an `Error` with a `code`, and with what the refusal names,
 * so that a page can word each one for the owner. None of it names a word
 * of the phrase.
 *
 * @module
 */

/** The `code` of a refusal by the identity module. */
export type PhraseErrorCode =
  /** Longer than any phrase could be. */
  | "tooLong"
  /** 12, 15, 18, 21 or 24 words: a BIP-39 phrase, which Dassie no longer reads; `words` says how many. */
  | "bip39"
  /** Not the 20 words of a phrase; `words` says how many. */
  | "wordCount"
  /** More wrong or unreadable words than the code restores. */
  | "tooDamaged"
  /** Kind 0: not a phrase. */
  | "invalidKind"
  /** A kind this version does not know, made by a newer Dassie; `kind` names it. */
  | "newerKind"
  /** A phrase of the other realm; `realm` names it. */
  | "otherRealm"
  /**
   * Made more than a day after this clock; `date` is the date the phrase
   * gives. This clock is behind, the words are not the phrase, or the clock
   * that made it ran ahead, and setting this clock past `date` reads it.
   */
  | "createdInFuture"
  /** Read only by a correction no one is there to confirm; `changed` names the positions, zero-based. */
  | "needsConfirmation"
  /** The clock is outside the hours a phrase can record. */
  | "clockOutOfRange"
  /** This clock and the node's are more than a day apart, so no phrase is made. */
  | "clocksApart"
  /** An argument names no realm. */
  | "realm"
  /** A position marked unreadable is not one of the 20. */
  | "unreadable"
  /** A dev phrase's secret is not 16 bytes. */
  | "secret";

/** A refusal by the identity module, with what it names. */
export interface PhraseError extends Error {
  code: PhraseErrorCode;
  /** For `bip39` and `wordCount`: how many words the text has. */
  words?: number;
  /** For `newerKind`: the kind the phrase names. */
  kind?: number;
  /** For `otherRealm`: the realm of the phrase. */
  realm?: "test" | "live";
  /** For `createdInFuture`: the date the phrase gives, `2026-02-19`. */
  date?: string;
  /** For `needsConfirmation`: the positions a correction would change. */
  changed?: Uint32Array;
}

const CODES: ReadonlySet<string> = new Set<PhraseErrorCode>([
  "tooLong",
  "bip39",
  "wordCount",
  "tooDamaged",
  "invalidKind",
  "newerKind",
  "otherRealm",
  "createdInFuture",
  "needsConfirmation",
  "clockOutOfRange",
  "clocksApart",
  "realm",
  "unreadable",
  "secret",
]);

/** Whether `error` is a refusal by the identity module. */
export function isPhraseError(error: unknown): error is PhraseError {
  return error instanceof Error && "code" in error &&
    typeof error.code === "string" && CODES.has(error.code);
}
