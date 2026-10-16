import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { linkKey, mutualLinks, PULSE_MS, Pulses } from "./graph.ts";

test("only mutual peerings are links, each once", () => {
  assertEquals(
    mutualLinks({
      d1: ["d2", "d3"],
      d2: ["d1"],
      d3: [],
      d4: ["d1"],
    }),
    [{ a: "d1", b: "d2" }],
  );
  assertEquals(
    mutualLinks({ d3: ["d1"], d1: ["d3", "d2"], d2: ["d1"] }),
    [{ a: "d1", b: "d2" }, { a: "d1", b: "d3" }],
  );
});

test("a pulse lights a link either way round, for a moment", () => {
  const pulses = new Pulses();
  pulses.hit("d2", "d1", 1_000);
  assertEquals([...pulses.lit()], [linkKey("d1", "d2")]);
  assertEquals(pulses.expire(1_000 + PULSE_MS - 1), false);
  assertEquals(pulses.expire(1_000 + PULSE_MS), true);
  assertEquals(pulses.lit().size, 0);
});
