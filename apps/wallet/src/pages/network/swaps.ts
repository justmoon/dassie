/**
 * What the network page says of the node's swaps with its peers
 * (spec/areas/connector.md#the-swap-service): whether it swaps by itself,
 * with how many peers, what moved, and when it waited for room of its own,
 * from the counters `GetSwapStatus` reports.
 */

import type { GetSwapStatusResponse } from "@dassie/rpc/proto";

/** The swaps' state in a word and a line, and their counters in a few. */
export interface SwapsView {
  badge: string;
  tone: "good" | "plain" | "bad";
  text: string;
  /** The counters, each a line; empty before the first swap. */
  details: string[];
}

/** `n` of `unit`, in the plural unless one. */
function count(n: bigint, unit: string): string {
  return `${n} ${unit}${n === 1n ? "" : "s"}`;
}

/**
 * What the page says of `status`, for a periphery node if `periphery`;
 * `undefined` while the node has not answered.
 */
export function swapsView(
  status: GetSwapStatusResponse | undefined,
  periphery: boolean,
): SwapsView | undefined {
  if (status === undefined) return undefined;
  const peers = count(status.peers, "peer");
  const badge = status.byItself ? "swapping" : "on request";
  const tone = status.byItself ? "good" : "plain";
  const text = status.byItself
    ? periphery
      ? `Your node swaps with its uplinks toward your targets, by the quotes of ${peers}.`
      : `Your node swaps with its peers toward an even split of what it holds, by the quotes of ${peers}.`
    : periphery
    ? "Your node swaps only when you ask: set targets in Settings to have it swap by itself."
    : "Your node swaps only when you ask: swapping is off in Settings.";
  if (status.sent === 0n) {
    return { badge, tone, text, details: [] };
  }
  const details = [
    `Sent ${
      count(status.sent, "swap")
    }: ${status.fulfilled} fulfilled, ${status.rejected} rejected, ${status.inFlight} in flight.`,
    `Backed off ${count(status.backoffs, "time")}; refused ${
      count(status.refused, "time")
    }; ${status.belowMinimum} came back short, ${status.tooLarge} too large.`,
  ];
  if (status.shortOfRoom > 0n) {
    details.push(
      `Waited ${
        count(status.shortOfRoom, "time")
      } for room of its own: a funding of its hold toward a peer, or its packets in flight to finish.`,
    );
  }
  if (status.waitingForRoom > 0n) {
    details.push(
      `Waiting for room of its own with ${
        count(status.waitingForRoom, "peer")
      } now: it swaps again as soon as there is.`,
    );
  }
  if (status.paidThoughRejected > 0n) {
    details.push(
      `${
        count(status.paidThoughRejected, "swap")
      } rejected after paying your node all the same.`,
    );
  }
  if (status.dropped > 0n) {
    details.push(
      `${count(status.dropped, "outcome")} dropped unread.`,
    );
  }
  return {
    badge,
    tone: status.dropped > 0n ? "bad" : tone,
    text,
    details,
  };
}
