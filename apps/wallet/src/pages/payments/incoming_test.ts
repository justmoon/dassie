import { assertEquals } from "@std/assert";
import {
  clientLabel,
  description,
  incomingState,
  newestFirst,
} from "./incoming.ts";

const payment = (id: string, createdAtMs: bigint) => ({
  id,
  client: "",
  metadataJson: "",
  createdAtMs,
  expiresAtMs: 2_000n,
});

Deno.test("an incoming payment is open until completed or expired", () => {
  assertEquals(incomingState({ expiresAtMs: 2_000n }, 1_999), "open");
  assertEquals(incomingState({ expiresAtMs: 2_000n }, 2_000), "expired");
  assertEquals(
    incomingState({ expiresAtMs: 2_000n, completedAtMs: 1_000n }, 3_000),
    "completed",
  );
});

Deno.test("payments are listed newest first, in a stable order", () => {
  const sorted = newestFirst([
    payment("b", 1n),
    payment("c", 2n),
    payment("a", 1n),
  ]);
  assertEquals(sorted.map((entry) => entry.id), ["c", "a", "b"]);
});

Deno.test("clients are named by their wallet address or their key", () => {
  assertEquals(clientLabel(""), "You");
  assertEquals(clientLabel("jwk:abc"), "A client known by its key");
  assertEquals(clientLabel("https://shop.example/pay"), "shop.example/pay");
  assertEquals(clientLabel("https://shop.example"), "shop.example");
  assertEquals(clientLabel("not a url"), "not a url");
});

Deno.test("the description comes from the metadata, when it is text", () => {
  assertEquals(description(""), undefined);
  assertEquals(description('{"description":"coffee"}'), "coffee");
  assertEquals(description('{"description":5}'), undefined);
  assertEquals(description("[1]"), undefined);
  assertEquals(description("{"), undefined);
});
