/**
 * Setup, login and logout: the node's three HTTP routes outside the RPC
 * contract (spec/areas/wallet.md#talking-to-the-node). The passphrase never
 * leaves the browser; setup sends the node key and the login proof derived from
 * it, login only the proof. A success sets or clears the session cookie, so
 * callers reload the page afterwards and let the gate pick the next screen.
 */

import { type Identity, loadIdentity } from "@dassie/wasm/web";
import { loginRequest, setupRequest } from "@dassie/rpc/auth";
import { hex } from "./hex.ts";
import type { Realm } from "./phrase.ts";

/** How a call to the node went. */
export type Outcome =
  | { readonly ok: true }
  | {
    readonly ok: false;
    /** The HTTP status, or 0 when the node could not be reached. */
    readonly status: number;
    /** What the node said, as plain text; may be empty. */
    readonly message: string;
    /** For 429: how long to wait, in seconds, if the node said. */
    readonly retryAfterSeconds?: number;
  };

/** What setup and login need from a passphrase. */
export interface Credentials {
  readonly nodeKey: Uint8Array;
  readonly loginProof: Uint8Array;
}

/**
 * The node key and login proof of a phrase of `realm`, in the canonical
 * form `readEntry` (`phrase.ts`) gives it once it read as written, or once
 * the owner confirmed its correction. Throws if it does not read as
 * written.
 */
export function deriveCredentials(
  identity: Identity,
  phrase: string,
  realm: Realm,
): Credentials {
  const read = identity.readPhrase(phrase, realm, Date.now()).exact();
  try {
    const nodeKey = read.nodeKey(realm);
    return { nodeKey, loginProof: identity.loginSecret(nodeKey) };
  } finally {
    read.free();
  }
}

/**
 * `POST /api/setup`: stores the node key of a phrase of `realm`, sets the
 * node up in that realm, and signs the owner in.
 */
export async function submitSetup(
  token: string,
  phrase: string,
  realm: Realm,
): Promise<Outcome> {
  const { nodeKey } = deriveCredentials(await loadIdentity(), phrase, realm);
  return await post("/api/setup", setupRequest(token, hex(nodeKey), realm));
}

/** `POST /api/login`: signs the owner in with the proof of a phrase of `realm`. */
export async function submitLogin(
  phrase: string,
  realm: Realm,
): Promise<Outcome> {
  const { loginProof } = deriveCredentials(
    await loadIdentity(),
    phrase,
    realm,
  );
  return await post("/api/login", loginRequest(hex(loginProof)));
}

/** `POST /api/login` with a proof as hex, for the dev environment's links. */
export function submitLoginProof(proofHex: string): Promise<Outcome> {
  return post("/api/login", loginRequest(proofHex));
}

/** `POST /api/logout`: ends this browser's session. */
export function submitLogout(): Promise<Outcome> {
  return post("/api/logout", {});
}

async function post(path: string, body: object): Promise<Outcome> {
  let response: Response;
  try {
    response = await fetch(path, {
      method: "POST",
      // The node accepts JSON only when it is labelled as JSON.
      headers: { "Content-Type": "application/json" },
      credentials: "same-origin",
      body: JSON.stringify(body),
    });
  } catch {
    return { ok: false, status: 0, message: "" };
  }
  if (response.ok) return { ok: true };
  const message = (await response.text().catch(() => "")).trim();
  const retryAfter = Number.parseInt(
    response.headers.get("Retry-After") ?? "",
    10,
  );
  return {
    ok: false,
    status: response.status,
    message,
    retryAfterSeconds: Number.isFinite(retryAfter) ? retryAfter : undefined,
  };
}
