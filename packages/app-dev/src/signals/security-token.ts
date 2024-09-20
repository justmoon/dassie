import { randomBytes } from "node:crypto"

import { createSignal } from "@dassie/lib-reactive"

/**
 * This token is used for authenticating RPC requests from the dev frontend to the nodes.
 *
 * @returns Signal containing a random 64 character hex string.
 */
export const SecurityTokenSignal = () =>
  createSignal(randomBytes(32).toString("hex"))
