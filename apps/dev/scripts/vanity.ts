/**
 * Keeps the dev nodes' identities (`apps/dev/vanity.json`,
 * spec/areas/dev.md#dev-identities).
 *
 * - `node --run vanity -- --check` derives every entry in both realms and
 *   fails on any that does not give its IDs.
 * - `node --run vanity -- --write` writes every entry's IDs again from its
 *   secret, as `dassie-identity` derives them: for when the derivation or
 *   the form of a node ID changes. The secrets stay.
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
  for (const [index, entry] of table) {
    derive(index, entry, "test");
    derive(index, entry, "live");
  }
  console.log(`${table.size} entries, each gives its IDs`);
} else if (mode === "--write") {
  // The IDs there may be of an older form: only the secret is read.
  const json: Record<string, string[]> = JSON.parse(
    await readFile(path, "utf8"),
  );
  const secrets = Object.fromEntries(
    Object.entries(json).map((
      [index, [secret]],
    ) => [index, [secret, idOf(secret, "test"), idOf(secret, "live")]]),
  );
  const written = parseTable(secrets);
  await writeFile(path, formatTable(written));
  console.log(`${written.size} entries written`);
} else {
  console.error("usage: node --run vanity -- --check | --write");
  process.exit(2);
}
