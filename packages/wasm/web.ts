/**
 * Identity derivation (`dassie-identity`) for browsers: the `gen/web` build,
 * which must instantiate its WASM module before any function works. The
 * functions are therefore handed out only by {@link loadIdentity}, once the
 * module is ready, so no caller can reach one too early.
 *
 * The module is fetched from next to the bindings (`new URL(…,
 * import.meta.url)`), which bundlers such as Vite recognise and emit as an
 * asset of their own, so the page loads it from its own origin.
 *
 * Build the bindings first with `node --run build:wasm`.
 *
 * @example
 * ```ts
 * const identity = await loadIdentity();
 * const made = identity.generatePhrase("test", Date.now(), nodeNowMs);
 * // Show made.words, and made.createdDate as `Generated 2026-02-19`.
 * const reading = identity.readPhrase(typed, "test", Date.now());
 * // If reading.corrected, show reading.words with reading.changed marked,
 * // and go on only once the owner confirms.
 * const nodeKey = reading.confirm().nodeKey("test");
 * ```
 *
 * @module
 */

import init, {
  generatePhrase,
  loginSecret,
  nodeId,
  type Phrase,
  type PhraseReading,
  phraseWords,
  publicKey,
  readPhrase,
} from "./gen/web/dassie_identity.js";

export {
  isPhraseError,
  type PhraseError,
  type PhraseErrorCode,
} from "./phrase_error.ts";
export type { Phrase, PhraseReading };

/** What the browser derives from a passphrase; the node derives the same. */
export interface Identity {
  /**
   * A new phrase of `realm` from the platform's random number generator,
   * made at `nowMs` (milliseconds since the Unix epoch). At setup, pass the
   * node's clock as `nodeNowMs`: no phrase is made while the two are more
   * than a day apart. Show the phrase with its `createdDate`. Throws a
   * {@link PhraseError}.
   */
  generatePhrase(
    realm: "test" | "live",
    nowMs: number,
    nodeNowMs?: number,
  ): Phrase;
  /**
   * Reads a phrase the owner typed, expecting one of `realm`, by a clock
   * that reads `nowMs`; the zero-based positions in `unreadable` are words
   * the owner marks as unreadable. A reading that is `corrected` must be
   * shown, with its `changed` positions marked, and `confirm`ed by the
   * owner before its phrase is used. Throws a {@link PhraseError}: its kind,
   * realm and creation time are checked before any correction is shown.
   */
  readPhrase(
    text: string,
    realm: "test" | "live",
    nowMs: number,
    unreadable?: Uint32Array,
  ): PhraseReading;
  /**
   * What each word of `text` reads as, in order, as {@link readPhrase}
   * splits and matches them: the word of the list it names by its first
   * four letters, or `undefined` for a word not on the list. Throws a
   * {@link PhraseError} (`tooLong`) beyond the longest input a reading
   * looks at.
   */
  phraseWords(text: string): (string | undefined)[];
  /**
   * The login secret of a node key: what setup and login send as proof.
   * Throws unless the key has 32 bytes.
   */
  loginSecret(nodeKey: Uint8Array): Uint8Array;
  /** The Ed25519 public key of a node key. Throws unless it has 32 bytes. */
  publicKey(nodeKey: Uint8Array): Uint8Array;
  /** The node ID of an Ed25519 public key. Throws unless it has 32 bytes. */
  nodeId(publicKey: Uint8Array): string;
}

let loading: Promise<Identity> | undefined;

/**
 * Instantiates the WASM module, once per page, and returns its functions.
 * A failed load is not remembered, so a later call tries again.
 */
export function loadIdentity(): Promise<Identity> {
  loading ??= init().then(
    (): Identity => ({
      generatePhrase,
      readPhrase,
      phraseWords,
      loginSecret,
      publicKey,
      nodeId,
    }),
    (error: unknown) => {
      loading = undefined;
      throw error;
    },
  );
  return loading;
}
