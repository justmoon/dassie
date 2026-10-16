import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { create } from "@bufbuild/protobuf";
import { LogLineSchema } from "@dassie/rpc/proto";
import { matches, parseFilter, splitIds, visibleRange } from "./filter.ts";

const line = create(LogLineSchema, {
  target: "das:node:peer-protocol",
  message: "Peered",
  fields: [{ name: "node", value: "d2_abc" }],
});

test("keywords must all match; negated ones must not", () => {
  assertEquals(matches(line, parseFilter("")), true);
  assertEquals(matches(line, parseFilter("peer")), true);
  assertEquals(matches(line, parseFilter("PEERED d2_")), true);
  assertEquals(matches(line, parseFilter("peer -protocol")), false);
  assertEquals(matches(line, parseFilter("-accounting")), true);
  assertEquals(matches(line, parseFilter("node=d2")), true);
  assertEquals(parseFilter("  a  -b - "), { include: ["a"], exclude: ["b"] });
});

test("only the rows in view, and a few around them, are rendered", () => {
  assertEquals(visibleRange(0, 200, 20, 1000), { start: 0, end: 20 });
  assertEquals(visibleRange(2000, 200, 20, 1000), { start: 90, end: 120 });
  assertEquals(visibleRange(19_900, 200, 20, 1000), { start: 985, end: 1000 });
  assertEquals(visibleRange(0, 200, 20, 0), { start: 0, end: 0 });
});

test("a line's source counts in the filter", () => {
  const line = create(LogLineSchema, {
    target: "das:node:http",
    message: "hi",
  });
  assertEquals(matches(line, parseFilter("d2"), "d2"), true);
  assertEquals(matches(line, parseFilter("d2"), "d1"), false);
  assertEquals(matches(line, parseFilter("-host"), "host"), false);
});

test("vanity IDs are picked out of text", () => {
  const id = "d12_abcdefghijklmnopqrstuvwxyz";
  assertEquals(splitIds(`peered with ${id}.`), [
    "peered with ",
    { id, name: "d12" },
    ".",
  ]);
  assertEquals(splitIds("no ids here"), ["no ids here"]);
  assertEquals(splitIds("d1_short"), ["d1_short"]);
});
