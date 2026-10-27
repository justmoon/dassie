/**
 * A peering's settlement methods (spec/areas/wallet.md#pages,
 * spec/decisions/settlement-methods.md): one for each settlement hub both
 * nodes are in, each with what is owed on it and how it stands.
 */

import type { NetworkPeer } from "@dassie/rpc/proto";

/** A settlement method as the details show it. */
export interface MethodRow {
  scheme: string;
  owed: bigint;
  withdrawn: boolean;
  windingDown: boolean;
}

/** The peering's methods, in the order the pair agreed them: those the node
 * lists, or the one it names for a peering it lists none for. */
export function peerMethods(peer: NetworkPeer): MethodRow[] {
  if (peer.methods.length > 0) {
    return peer.methods.map((method) => ({
      scheme: method.scheme,
      owed: method.owed,
      withdrawn: method.withdrawn,
      windingDown: method.windingDown,
    }));
  }
  return [{
    scheme: peer.scheme,
    owed: peer.owed,
    withdrawn: false,
    windingDown: false,
  }];
}

/** How a method stands, when it is not simply in use: being wound down, or
 * short of room. */
export function methodState(
  method: MethodRow,
): { label: string; tone: "bad" | "plain"; hint: string } | undefined {
  if (method.windingDown) {
    return {
      label: "Winding down",
      tone: "plain",
      hint:
        "Your node takes no new payments on it. It asks the peer to settle once, then stops settling with it there.",
    };
  }
  if (method.withdrawn) {
    return {
      label: "Short of room",
      tone: "bad",
      hint:
        "The peer has not settled for a while, so no payments go through it here until it does.",
    };
  }
  return undefined;
}
