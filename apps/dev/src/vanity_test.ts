import { test } from "node:test";
import { assertEquals, assertNotEquals, assertThrows } from "@std/assert";
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
  "4lhg2yuwje6ai2ly6suw42foizfxpkbrigaowiy",
  "mlxq6rukr7si7vwe32j7m4d5gkxlctpkq6pyhgy",
];

/** d1's phrase in the test realm. */
const PHRASE_D1 =
  "advice abandon approve resource position trend piano across machine scorpion defy explain increase addict into uncle subject comic dolphin convince";

test("the table's first nodes derive their IDs", () => {
  const table = loadTable();
  // Every entry is checked by `node --run vanity -- --check`; a sample here.
  const named = names(table);
  for (const index of [0, 1, 2, 5, 99]) {
    const node = identity(table, index);
    assertEquals(node.name, `d${index + 1}`);
    assertEquals(node.realm, "test");
    assertEquals(named.get(node.id), node.name);
    assertEquals(node.loginProofHex.length, 64);
    // The same secret is another node in the live realm.
    const live = identity(table, index, "live");
    assertEquals(live.realm, "live");
    assertEquals(named.get(live.id), node.name);
    assertNotEquals(live.id, node.id);
    assertNotEquals(live.phrase, node.phrase);
  }
  assertEquals(table.get(0), ENTRY_D1);
  assertEquals(identity(table, 0).phrase, PHRASE_D1);
});

test("an entry that does not give its IDs is refused", () => {
  assertThrows(() => derive(0, [ENTRY_D1[0], "a".repeat(39), ENTRY_D1[2]]));
  // Each realm's ID is checked on its own.
  assertThrows(() =>
    derive(0, [ENTRY_D1[0], ENTRY_D1[1], ENTRY_D1[1]], "live")
  );
  assertThrows(() => identity(new Map(), 0));
  assertEquals(idOf(ENTRY_D1[0], "test"), ENTRY_D1[1]);
  assertEquals(idOf(ENTRY_D1[0], "live"), ENTRY_D1[2]);
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
  assertThrows(() => parseTable({ "1": ["xyz", ENTRY_D1[1], ENTRY_D1[2]] }));
  assertThrows(() => parseTable({ "1": [ENTRY_D1[0], "d2_a", ENTRY_D1[2]] }));
  assertThrows(() => parseTable({ "1": [ENTRY_D1[0], ENTRY_D1[1], "d2_a"] }));
  // The old form, one ID for both realms.
  assertThrows(() => parseTable({ "1": [ENTRY_D1[0], ENTRY_D1[1]] }));
});
