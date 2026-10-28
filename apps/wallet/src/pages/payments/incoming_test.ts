import { test } from "node:test";
import { assertEquals } from "@std/assert";
import {
  clientLabel,
  description,
  incomingState,
  newestFirst,
  shownReceived,
} from "./incoming.ts";

const payment = (id: string, createdAtUnixMs: bigint) => ({
  id,
  client: "",
  metadataJson: "",
  createdAtUnixMs,
  expiresAtUnixMs: 2_000n,
});

test("an incoming payment is open until completed or expired", () => {
  assertEquals(incomingState({ expiresAtUnixMs: 2_000n }, 1_999), "open");
  assertEquals(incomingState({ expiresAtUnixMs: 2_000n }, 2_000), "expired");
  assertEquals(
    incomingState(
      { expiresAtUnixMs: 2_000n, completedAtUnixMs: 1_000n },
      3_000,
    ),
    "completed",
  );
});

test("a payment paid in full shows its amount, else what arrived", () => {
  const asked = { incomingAmount: 25_000_000_000n };
  assertEquals(
    shownReceived({
      ...asked,
      receivedAmount: 24_999_900_000n,
      paidInFull: true,
    }),
    25_000_000_000n,
  );
  assertEquals(
    shownReceived({
      ...asked,
      receivedAmount: 25_000_100_000n,
      paidInFull: true,
    }),
    25_000_000_000n,
  );
  assertEquals(
    shownReceived({
      ...asked,
      receivedAmount: 12_000_000_000n,
      paidInFull: false,
    }),
    12_000_000_000n,
  );
  assertEquals(
    shownReceived({ receivedAmount: 7n, paidInFull: false }),
    7n,
  );
});

test("payments are listed newest first, in a stable order", () => {
  const sorted = newestFirst([
    payment("b", 1n),
    payment("c", 2n),
    payment("a", 1n),
  ]);
  assertEquals(sorted.map((entry) => entry.id), ["c", "a", "b"]);
});

test("clients are named by their wallet address or their key", () => {
  assertEquals(clientLabel(""), "You");
  assertEquals(clientLabel("jwk:abc"), "A client known by its key");
  assertEquals(clientLabel("https://shop.example/pay"), "shop.example/pay");
  assertEquals(clientLabel("https://shop.example"), "shop.example");
  assertEquals(clientLabel("not a url"), "not a url");
});

test("the description comes from the metadata, when it is text", () => {
  assertEquals(description(""), undefined);
  assertEquals(description('{"description":"coffee"}'), "coffee");
  assertEquals(description('{"description":5}'), undefined);
  assertEquals(description("[1]"), undefined);
  assertEquals(description("{"), undefined);
});
