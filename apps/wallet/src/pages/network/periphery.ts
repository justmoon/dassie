/**
 * What the network page says of a periphery node
 * (spec/areas/network.md#periphery-nodes): a node without a public address
 * registers with no bootstrap node, and its uplinks do not list it; it reaches
 * the network through its uplinks, and Open Payments clients reach it
 * through its relay (spec/areas/node.md#a-chosen-relay).
 */

import { type NetworkPeer, RelayConnection } from "@dassie/rpc/proto";
import { relativeTime } from "@dassie/ui/format";
import type { Relay } from "../payments/front_door.ts";

/** How an uplink stands, in a few words. */
export function uplinkState(
  peering: NetworkPeer | undefined,
  nowMs: number,
): string {
  if (peering?.online) return "connected";
  const lastSeen = Number(peering?.lastSeenUnixMs ?? 0n);
  if (lastSeen > 0) {
    return `not connected, last heard from ${relativeTime(lastSeen, nowMs)}`;
  }
  return "not connected yet";
}

/** Whether an uplink is connected, for its badge's tone. */
export function uplinkConnected(peering: NetworkPeer | undefined): boolean {
  return peering?.online === true;
}

/** What the page says of a node without uplinks. */
export const NO_UPLINKS =
  "None yet, so no payment reaches this node: it peers with core nodes that share one of its ledgers.";

/**
 * How the node stands with its relay, given what it reports: `null` when it
 * keeps none.
 */
export function relayState(relay: Relay | null): {
  text: string;
  tone: "good" | "plain" | "bad";
} {
  if (relay === null) {
    return {
      text:
        "None (DASSIE_RELAY is empty), so Open Payments clients cannot reach this node.",
      tone: "bad",
    };
  }
  switch (relay.connection) {
    case RelayConnection.CONNECTED:
      return { text: "connected", tone: "good" };
    case RelayConnection.CONNECTING:
      return { text: "connecting", tone: "plain" };
    default:
      // UNREACHABLE, or a state this wallet does not know.
      return {
        text:
          "unreachable, so Open Payments clients cannot reach this node through it",
        tone: "bad",
      };
  }
}
