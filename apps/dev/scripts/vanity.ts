/**
 * Keeps the dev nodes' identities (`apps/dev/vanity.json`,
 * spec/areas/dev.md#dev-identities).
 *
 * - `node --run vanity -- --check` derives every entry and fails on any that
 *   does not give its ID.
 * - `node --run vanity -- --write` writes every entry's ID again from its
 *   entropy, as `dassie-identity` derives it: for when the form of a node ID
 *   changes. The keys stay.
 */

import { readFile, writeFile } from "node:fs/promises";
import process from "node:process";
import {
  derive,
  formatTable,
  idOf,
  loadTable,
  parseTable,
} from "../src/vanity.ts";

const path = new URL("../vanity.json", import.meta.url);
const [mode] = process.argv.slice(2);

if (mode === "--check") {
  const table = loadTable();
  for (const [index, entry] of table) derive(index, entry);
  console.log(`${table.size} entries, each gives its ID`);
} else if (mode === "--write") {
  // The IDs there may be of an older form: only the entropy is read.
  const json: Record<string, string[]> = JSON.parse(
    await readFile(path, "utf8"),
  );
  const entropies = Object.fromEntries(
    Object.entries(json).map((
      [index, [entropy]],
    ) => [index, [entropy, idOf(entropy)]]),
  );
  const written = parseTable(entropies);
  await writeFile(path, formatTable(written));
  console.log(`${written.size} entries written`);
} else {
  console.error("usage: node --run vanity -- --check | --write");
  process.exit(2);
}
