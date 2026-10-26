import { test } from "node:test";
import { assertEquals, assertThrows } from "@std/assert";
import {
  derive,
  type Entry,
  formatTable,
  identity,
  idOf,
  loadTable,
  MAX_INDEX,
  names,
  parseTable,
} from "./vanity.ts";

const ENTRY_D1: Entry = [
  "ad6f2a1f40a4004a1660939aa11ca81a",
  "272r4ccpkqycyl2uoslxjd3i2e5hhyvz4j2wxui",
];

test("the table's first nodes derive their IDs", () => {
  const table = loadTable();
  // Every entry is checked by `node --run vanity -- --check`; a sample here.
  const named = names(table);
  for (const index of [0, 1, 2, 5, 99]) {
    const node = identity(table, index);
    assertEquals(node.name, `d${index + 1}`);
    assertEquals(named.get(node.id), node.name);
    assertEquals(node.loginProofHex.length, 64);
  }
  assertEquals(table.get(0), ENTRY_D1);
});

test("an entry that does not give its ID is refused", () => {
  assertThrows(() => derive(0, [ENTRY_D1[0], "a".repeat(39)]));
  assertThrows(() => identity(new Map(), 0));
  assertEquals(idOf(ENTRY_D1[0]), ENTRY_D1[1]);
});

test("the table holds an entry for every node, and no more", () => {
  const table = loadTable();
  assertEquals(table.size, MAX_INDEX + 1);
  assertEquals(Math.max(...table.keys()), MAX_INDEX);
});

test("the table's form", () => {
  const table = new Map<number, Entry>([[10, ENTRY_D1], [2, ENTRY_D1]]);
  const text = formatTable(table);
  assertEquals(
    parseTable(JSON.parse(text)),
    new Map([[2, ENTRY_D1], [10, ENTRY_D1]]),
  );
  assertEquals(text.indexOf('"2"') < text.indexOf('"10"'), true);
  assertThrows(() => parseTable({ "01": [...ENTRY_D1] }));
  assertThrows(() => parseTable({ "1000": [...ENTRY_D1] }));
  assertThrows(() => parseTable({ "1": ["xyz", ENTRY_D1[1]] }));
  assertThrows(() => parseTable({ "1": [ENTRY_D1[0], "d2_a"] }));
});
