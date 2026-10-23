import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { create } from "@bufbuild/protobuf";
import { RateFeedSchema } from "@dassie/rpc/proto";
import { feedView } from "./feed.ts";

test("a served feed says where, to how many, and what it sent", () => {
  const feed = create(RateFeedSchema, {
    serving: true,
    address: "0.0.0.0:7768",
    subscribers: 1,
    published: 12n,
    sent: 30n,
    received: 40n,
    challenged: 3n,
    subscribed: 2n,
    renewed: 33n,
    refusedFull: 1n,
    ignored: 1n,
    expired: 1n,
  });
  assertEquals(feedView(feed), {
    badge: "served",
    tone: "good",
    text: "At 0.0.0.0:7768, to 1 subscriber.",
    details: [
      "Published 12 feeds; sent 30 datagrams, 0 failed, 0 out of time.",
      "Took 40 datagrams: 3 challenged, 2 subscribed, 33 renewed, 1 ignored; 1 lapsed.",
      "Refused 1 for a full table, 0 for a client's share.",
    ],
  });
  // A send that failed shows.
  const failed = feedView({ ...feed, sendFailed: 2n });
  assertEquals([failed.badge, failed.tone], ["sends failed", "bad"]);
});

test("a feed that does not serve, or no socket, says why", () => {
  const waiting = create(RateFeedSchema, { address: "0.0.0.0:7768" });
  assertEquals(feedView(waiting), {
    badge: "not served",
    tone: "plain",
    text: "The node publishes it at 0.0.0.0:7768 once it has a domain name.",
    details: [],
  });
  assertEquals(feedView(undefined).badge, "none");
});
