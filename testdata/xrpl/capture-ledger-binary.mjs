// Captures crates/dassie-xrpl/tests/data/ledger-binary.json
// (spec/designs/xrpl-validation.md#13-what-pins-it), read-only: some
// transactions of a mainnet ledger with their metadata, from `ledger` with
// transactions, expand and binary, and ledger objects from `ledger_entry`
// with binary: the XRP/RLUSD pool's account root, its RLUSD trust line and
// its AMM object, and RLUSD's issuer's account root. Unless given, the
// transactions are the fewest, first in the ledger's order, that with the
// objects hold every serialized type the test expects, of the newest
// ledger whose transactions do.
//
// Usage, from the repository root, at recent ledgers or given ones
// (transactions by their place in the ledger's list, from 0):
//
//   node testdata/xrpl/capture-ledger-binary.mjs [--ledger <index>] \
//     [--transactions <i>,<j>,...] [--objects <index>] \
//     > crates/dassie-xrpl/tests/data/ledger-binary.json
//
// XRPL_MAINNET names another server (capture-lib.mjs); an older ledger
// needs one that keeps its history.

import {
  assert,
  fields,
  ledgerHeader,
  ledgerOption,
  MAINNET,
  open,
  options,
  POOL_LABELS,
  poolObjects,
  print,
  transactions,
  unhex,
} from "./capture-lib.mjs";

// The types the test expects the blobs to hold, by code: UInt16 to
// AccountID, Object, Array, UInt8, Hash160, PathSet, Vector256 and Issue.
const EXPECTED = [1, 2, 3, 4, 5, 6, 7, 8, 14, 15, 16, 17, 18, 19, 24];
const MAX_PICKED = 16;
const LEDGERS_BACK = 200;

const given = options(["ledger", "transactions", "objects"]);
const captured = new Date().toISOString();
const connection = await open(MAINNET);
const newest = await ledgerHeader(connection, ledgerOption(given.ledger));
const pool = await poolObjects(connection, given.objects === undefined ? newest.index : ledgerOption(given.objects));
const objects = Object.entries(POOL_LABELS).map(([key, label]) => ({ label, hex: pool[key].node_binary }));
const typesOf = (blobs) => new Set(blobs.flatMap((blob) => [...fields(unhex(blob)).types]));
const byObjects = typesOf(objects.map((object) => object.hex));

// The transactions given, or greedily, each time the one that holds the
// most types still missing, the first of equals; none if the ledger lacks
// a type.
function pick(entries) {
  const missing = new Set(EXPECTED.filter((type) => !byObjects.has(type)));
  const types = entries.map(({ tx_blob, meta }) => typesOf([tx_blob, meta]));
  if (given.transactions !== undefined) {
    const picked = given.transactions.split(",").map(Number);
    assert(picked.every((at) => Number.isInteger(at) && at >= 0 && at < entries.length), "transactions in the ledger");
    for (const at of picked) for (const type of types[at]) missing.delete(type);
    assert(missing.size === 0, `the blobs hold every type the test expects, but ${[...missing]}`);
    return picked;
  }
  const picked = [];
  while (missing.size > 0) {
    assert(picked.length < MAX_PICKED, "few transactions");
    let best = -1;
    let gain = 0;
    types.forEach((held, at) => {
      const adds = [...held].filter((type) => missing.has(type)).length;
      if (adds > gain) [best, gain] = [at, adds];
    });
    if (best < 0) return null;
    picked.push(best);
    for (const type of types[best]) missing.delete(type);
  }
  return picked.sort((a, b) => a - b);
}

// The ledger given, or the newest of the last few that holds every type.
let header = newest;
let entries = await transactions(connection, header.index);
let picked = pick(entries);
for (let back = 1; picked === null && given.ledger === undefined && back < LEDGERS_BACK; back++) {
  header = await ledgerHeader(connection, newest.index - back);
  entries = await transactions(connection, header.index);
  picked = pick(entries);
}
assert(picked !== null, `ledger ${header.index} lacks a type the test expects: give another`);
connection.close();

const blobs = picked.flatMap((at) => [
  { label: `ledger ${header.index}, transaction ${at}, tx_blob`, hex: entries[at].tx_blob },
  { label: `ledger ${header.index}, transaction ${at}, meta`, hex: entries[at].meta },
]);
print({
  source: `Transactions and metadata of mainnet ledger ${header.index} (ledger with transactions, expand and binary), and ledger objects from ledger_entry with binary as of ledger ${pool.root.ledger_index}: the XRP/RLUSD pool's account root, its RLUSD trust line and its AMM object, and RLUSD's issuer's account root. Captured ${captured} from ${MAINNET} (rippled API v1) by testdata/xrpl/capture-ledger-binary.mjs.`,
  blobs: [...blobs, ...objects],
});
