/** The RPC client every page talks to the node through. */

import type { RpcClient } from "@dassie/rpc";
import { createContext, useContext } from "solid-js";

/** The node's client, for the subtree. */
export const RpcContext = createContext<RpcClient>();

/** The node's client. */
export function useClient(): RpcClient {
  return useContext(RpcContext);
}

/** A failure as people read it. */
export function describe(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error);
}
