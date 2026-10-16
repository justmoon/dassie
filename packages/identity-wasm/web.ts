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
 * const phrase = identity.generateMnemonic();
 * const nodeKey = identity.deriveNodeKey(phrase);
 * ```
 *
 * @module
 */

import init, {
  deriveNodeKey,
  generateMnemonic,
  loginSecret,
  nodeId,
  normalizeMnemonic,
  publicKey,
  validateMnemonic,
} from "./gen/web/dassie_identity.js";

/** What the browser derives from a passphrase; the node derives the same. */
export interface Identity {
  /** A new 12-word phrase from the platform's random number generator. */
  generateMnemonic(): string;
  /**
   * Whether `phrase` is a valid BIP-39 phrase: known words and a matching
   * checksum.
   */
  validateMnemonic(phrase: string): boolean;
  /**
   * The phrase in canonical form: lowercase, single spaces. Throws if the
   * phrase is not valid.
   */
  normalizeMnemonic(phrase: string): string;
  /**
   * The node key (Ed25519 seed, 32 bytes) of a phrase. Throws if the phrase
   * is not valid.
   */
  deriveNodeKey(phrase: string): Uint8Array;
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
      generateMnemonic,
      validateMnemonic,
      normalizeMnemonic,
      deriveNodeKey,
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
