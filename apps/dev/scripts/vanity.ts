/**
 * Keeps the vanity table (`apps/dev/vanity.json`, spec 6 §6.4).
 *
 * - `node --run vanity < found.txt` adds a brute-forcer's finds, one
 *   `[0x]<entropy hex>: <node ID>` a line (`#` starts a comment), by the
 *   original's rules (`merge` in src/vanity.ts). Each is derived first and
 *   refused if its entropy does not give its ID.
 * - `node --run vanity -- --check` derives every entry and fails on any that
 *   does not give its ID.
 *
 * The table began as the original's `TEST_NODE_VANITY_ENTROPY`
 * (packages/app-dev/src/constants/vanity-nodes.ts), fed through the first
 * form: `N: "<entropy>", // <ID>` lines become `<entropy>: <ID>`.
 */

import { writeFile } from "node:fs/promises";
import process from "node:process";
import { createInterface } from "node:readline";
import {
  derive,
  type Entry,
  formatTable,
  indexOf,
  loadTable,
  merge,
  parseFound,
} from "../src/vanity.ts";

const path = new URL("../vanity.json", import.meta.url);
const table = loadTable();

if (process.argv.slice(2).includes("--check")) {
  for (const [index, entry] of table) derive(index, entry);
  console.log(`${table.size} entries, each gives its ID`);
  process.exit(0);
}

const found: Entry[] = [];
const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
for await (const line of lines) {
  const entry = parseFound(line);
  if (!entry) continue;
  const index = indexOf(entry[1]);
  if (index === undefined) throw new Error(`not a vanity ID: ${entry[1]}`);
  derive(index, entry);
  found.push(entry);
}
const merged = merge(table, found);
await writeFile(path, formatTable(merged));
console.log(`${found.length} found, ${merged.size} entries`);
