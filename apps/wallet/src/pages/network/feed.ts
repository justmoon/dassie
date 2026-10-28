/**
 * What the network page says of a core node's UDP endpoint for its snapshot
 * (spec/areas/rate-feed.md#what-the-owner-sees): whether the node answers
 * there, and what it took and answered, from the counters `GetStatus`
 * reports.
 */

import type { RateFeed } from "@dassie/rpc/proto";

/** The feed's state in a word and a line, and its counters in a few. */
export interface FeedView {
  badge: string;
  tone: "good" | "plain" | "bad";
  text: string;
  /** The counters, each a line; empty while the feed does not serve. */
  details: string[];
}

/** `n` of `unit`, in the plural unless one. */
function count(n: bigint | number, unit: string): string {
  return `${n} ${unit}${BigInt(n) === 1n ? "" : "s"}`;
}

/** What the page says of `feed`, `undefined` when the node has no socket. */
export function feedView(feed: RateFeed | undefined): FeedView {
  if (feed === undefined) {
    return {
      badge: "none",
      tone: "bad",
      text:
        "The node has no UDP socket for it, so it answers no request for its snapshot.",
      details: [],
    };
  }
  if (!feed.serving) {
    return {
      badge: "not answering",
      tone: "plain",
      text:
        `The node answers at ${feed.address} once it has a domain name and offers swaps.`,
      details: [],
    };
  }
  const failed = feed.sendFailed > 0n;
  return {
    badge: failed ? "sends failed" : "answering",
    tone: failed ? "bad" : "good",
    text: `At ${feed.address}.`,
    details: [
      `Took ${
        count(feed.received, "datagram")
      }: answered ${feed.answered}, ${feed.overBudget} over the budget, ${feed.ignored} ignored; ${feed.sendFailed} sends failed.`,
    ],
  };
}
