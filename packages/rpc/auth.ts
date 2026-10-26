/**
 * The bodies of the node's setup and login routes, `POST /api/setup` and
 * `POST /api/login`, as the node reads them (`SetupRequest` and
 * `LoginRequest` in crates/dassie-node-core/src/setup.rs,
 * spec/areas/http.md#setup). Everything that sets a node up or logs in to
 * one writes them here: the wallet, the dev environment and the tests.
 *
 * @module
 */

/** The body of `POST /api/setup`. */
export interface SetupRequest {
  /** The token from the setup URL. */
  readonly setupToken: string;
  /** The node key, 32 bytes as hex. */
  readonly nodeKey: string;
  /**
   * The realm to set the node up in, `test` if absent. Fixed for good once
   * the node is set up.
   */
  readonly realm?: "test" | "live";
}

/** The body of `POST /api/login`. */
export interface LoginRequest {
  /** The login proof of the node key, 32 bytes as hex. */
  readonly loginProof: string;
}

/**
 * The setup body of the token from the setup URL and a node key as hex, in
 * `realm` if given.
 */
export function setupRequest(
  setupToken: string,
  nodeKeyHex: string,
  realm?: "test" | "live",
): SetupRequest {
  return realm === undefined
    ? { setupToken, nodeKey: nodeKeyHex }
    : { setupToken, nodeKey: nodeKeyHex, realm };
}

/** The login body of a login proof as hex. */
export function loginRequest(loginProofHex: string): LoginRequest {
  return { loginProof: loginProofHex };
}
