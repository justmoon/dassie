/**
 * What the network page says of a core node's rate feed
 * (spec/areas/rate-feed.md#what-the-owner-sees): whether the node serves it,
 * to how many, and what it sent, from the counters `GetStatus` reports.
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
      text: "The node has no UDP socket for it, so it publishes no feed.",
      details: [],
    };
  }
  if (!feed.serving) {
    return {
      badge: "not served",
      tone: "plain",
      text:
        `The node publishes it at ${feed.address} once it has a domain name.`,
      details: [],
    };
  }
  const failed = feed.sendFailed + feed.unsent > 0n;
  return {
    badge: failed ? "sends failed" : "served",
    tone: failed ? "bad" : "good",
    text: `At ${feed.address}, to ${count(feed.subscribers, "subscriber")}.`,
    details: [
      `Published ${count(feed.published, "feed")}; sent ${
        count(feed.sent, "datagram")
      }, ${feed.sendFailed} failed, ${feed.unsent} out of time.`,
      `Took ${
        count(feed.received, "datagram")
      }: ${feed.challenged} challenged, ${feed.subscribed} subscribed, ${feed.renewed} renewed, ${feed.ignored} ignored; ${feed.expired} lapsed.`,
      `Refused ${feed.refusedFull} for a full table, ${feed.refusedClient} for a client's share.`,
    ],
  };
}
