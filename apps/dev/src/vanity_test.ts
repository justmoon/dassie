import { test } from "node:test";
import { assertEquals, assertThrows } from "@std/assert";
import {
  derive,
  type Entry,
  formatTable,
  identity,
  indexOf,
  loadTable,
  merge,
  parseFound,
  parseTable,
} from "./vanity.ts";

const D1: Entry = [
  "ad6f2a1f40a4004a1660939aa11ca81a",
  "d1_UeCE9UMCwvVHSXdI9o0Tpz4rnidWvR",
];

test("the table's first nodes derive their IDs", () => {
  const table = loadTable();
  // Every entry is checked by `deno task vanity --check`; a sample here.
  for (const index of [0, 1, 2, 5, 99, 999]) {
    const node = identity(table, index);
    assertEquals(node.name, `d${index + 1}`);
    assertEquals(indexOf(node.id), index);
    assertEquals(node.loginProofHex.length, 64);
  }
});

test("an entry that does not give its ID is refused", () => {
  assertThrows(() => derive(0, [D1[0], "d1_somethingElse"]));
  assertThrows(() => identity(new Map(), 0));
});

test("an ID's index is its number less one", () => {
  assertEquals(indexOf("d1_x"), 0);
  assertEquals(indexOf("d1000_x"), 999);
  assertEquals(indexOf("d0_x"), undefined);
  assertEquals(indexOf("x1_y"), undefined);
});

test("merge keeps new indexes and less noisy IDs", () => {
  const table = new Map<number, Entry>([[0, ["a".repeat(32), "d1_a-b_c"]]]);
  const merged = merge(table, [
    ["b".repeat(32), "d1_abc"],
    ["c".repeat(32), "d2_x"],
    ["d".repeat(32), "d2_x-y"],
  ]);
  assertEquals(merged.get(0), ["b".repeat(32), "d1_abc"]);
  assertEquals(merged.get(1), ["c".repeat(32), "d2_x"]);
});

test("merge drops indexes beyond 999 but the record", () => {
  const merged = merge(new Map(), [
    ["a".repeat(32), "d5000_a"],
    ["b".repeat(32), "d7000_b"],
    ["c".repeat(32), "d1000_c"],
  ]);
  assertEquals([...merged.keys()].sort((a, b) => a - b), [999, 6999]);
});

test("brute-forcer lines and the table's form", () => {
  assertEquals(parseFound(`0x${D1[0].toUpperCase()}: ${D1[1]}`), D1);
  assertEquals(parseFound("# a comment"), undefined);
  assertEquals(parseFound(""), undefined);
  assertThrows(() => parseFound("nonsense"));
  const table = new Map<number, Entry>([[10, D1], [2, D1]]);
  const text = formatTable(table);
  assertEquals(parseTable(JSON.parse(text)), new Map([[2, D1], [10, D1]]));
  assertEquals(text.indexOf('"2"') < text.indexOf('"10"'), true);
  assertThrows(() => parseTable({ "01": [...D1] }));
  assertThrows(() => parseTable({ "1": ["xyz", "d2_a"] }));
});
