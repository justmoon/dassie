import { assertEquals } from "@std/assert";
import { applyTableEvent, emptyKeyed, pushRing } from "./reduce.ts";

interface Node {
  id: string;
  alias: string;
}

const key = (node: Node) => node.id;

Deno.test("a snapshot, then upserts and removals", () => {
  const table = emptyKeyed<Node>();
  applyTableEvent(table, {
    reset: true,
    upserted: [{ id: "a", alias: "" }, { id: "b", alias: "" }],
    removed: [],
  }, key);
  assertEquals(table.ready, true);
  applyTableEvent(table, {
    reset: false,
    upserted: [{ id: "a", alias: "alpha" }],
    removed: ["b"],
  }, key);
  assertEquals(table.byId, { a: { id: "a", alias: "alpha" } });
});

// A new subscription's snapshot drops what left while the stream was
// down.
Deno.test("a new snapshot replaces what was held", () => {
  const table = emptyKeyed<Node>();
  const snapshot = (ids: string[]) =>
    applyTableEvent(table, {
      reset: true,
      upserted: ids.map((id) => ({ id, alias: "" })),
      removed: [],
    }, key);
  snapshot(["a", "b"]);
  snapshot(["b", "c"]);
  assertEquals(Object.keys(table.byId).sort(), ["b", "c"]);
});

Deno.test("a ring keeps its newest entries", () => {
  const ring = [1, 2, 3];
  assertEquals(pushRing(ring, [4, 5], 4), 1);
  assertEquals(ring, [2, 3, 4, 5]);
});
