import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { create } from "@bufbuild/protobuf";
import { RateFeedSchema } from "@dassie/rpc/proto";
import { feedView } from "./feed.ts";

test("an endpoint that answers says where, and what it took and answered", () => {
  const feed = create(RateFeedSchema, {
    serving: true,
    address: "0.0.0.0:7768",
    received: 40n,
    answered: 36n,
    overBudget: 3n,
    ignored: 1n,
  });
  assertEquals(feedView(feed), {
    badge: "answering",
    tone: "good",
    text: "At 0.0.0.0:7768.",
    details: [
      "Took 40 datagrams: answered 36, 3 over the budget, 1 ignored; 0 sends failed.",
    ],
  });
  // A send that failed shows.
  const failed = feedView({ ...feed, sendFailed: 2n });
  assertEquals([failed.badge, failed.tone], ["sends failed", "bad"]);
});

test("an endpoint that does not answer, or no socket, says why", () => {
  const waiting = create(RateFeedSchema, { address: "0.0.0.0:7768" });
  assertEquals(feedView(waiting), {
    badge: "not answering",
    tone: "plain",
    text:
      "The node answers at 0.0.0.0:7768 once it has a domain name and offers swaps.",
    details: [],
  });
  assertEquals(feedView(undefined).badge, "none");
});
