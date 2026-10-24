// Captures crates/dassie-xrpl/tests/data/transaction-set.json
// (spec/designs/xrpl-validation.md#13-what-pins-it), read-only: a mainnet
// ledger's header and every transaction with its metadata, from `ledger`
// with transactions, expand and binary; one transaction's branch in its
// transaction tree, each inner node on the path as its sixteen children's
// hashes, root first; the XRP/RLUSD pool's four ledger objects, from
// `ledger_entry` with binary; and the branches of the transactions that
// last changed two of them, the pool's account root and its AMM object,
// each from `tx` with binary, with its ledger's header.
//
// The trees are built here as rippled builds them, and each root must be
// the one its header names.
//
// Usage, from the repository root, at the validated ledger or given ones
// (the branch's transaction by its place in the ledger's list, from 0; by
// default the first whose leaf hangs below the root's children):
//
//   node testdata/xrpl/capture-transaction-set.mjs [--ledger <index>] \
//     [--branch <i>] [--objects <index>] \
//     > crates/dassie-xrpl/tests/data/transaction-set.json
//
// The other branches must be of ledgers before the first: if the pool
// changed in it, give the objects' ledger as one before it. XRPL_MAINNET
// names another server (capture-lib.mjs); an older ledger needs one that
// keeps its history.

import {
  assert,
  branchLevels,
  fields,
  hex,
  ledgerHeader,
  ledgerOption,
  MAINNET,
  open,
  options,
  POOL_LABELS,
  poolObjects,
  print,
  transactionId,
  transactions,
  transactionTree,
  unhex,
} from "./capture-lib.mjs";

const HASH256 = 5;
const PREVIOUS_TXN_ID = 5;

// The transaction that last changed an object.
function previousTransaction(nodeBinary) {
  const bytes = unhex(nodeBinary);
  const field = fields(bytes).list.find(({ type, field }) => type === HASH256 && field === PREVIOUS_TXN_ID);
  assert(field !== undefined, "an object threaded to its last transaction");
  return hex(bytes.subarray(field.valueStart, field.end));
}

// A ledger's header and transaction tree, whose root the header names.
async function ledgerTree(connection, ledgerIndex) {
  const header = await ledgerHeader(connection, ledgerIndex);
  const entries = await transactions(connection, header.index);
  const tree = transactionTree(entries);
  assert(hex(tree.hash) === header.json.transaction_hash, `the tree of ledger ${header.index} has its root`);
  return { header, entries, tree };
}

const given = options(["ledger", "branch", "objects"]);
const captured = new Date().toISOString();
const connection = await open(MAINNET);
const { header, entries, tree } = await ledgerTree(connection, ledgerOption(given.ledger));
const keys = entries.map(({ tx_blob }) => transactionId(tx_blob));
let at;
if (given.branch !== undefined) {
  at = Number(given.branch);
  assert(Number.isInteger(at) && at >= 0 && at < entries.length, "a transaction in the ledger");
} else {
  at = keys.findIndex((key) => branchLevels(tree, key).levels.length >= 2);
  assert(at >= 0, `ledger ${header.index} has a leaf below the root's children: give another`);
}
const branch = branchLevels(tree, keys[at]);

const pool = await poolObjects(connection, given.objects === undefined ? header.index : ledgerOption(given.objects));
const objects = Object.entries(POOL_LABELS).map(([key, label]) => ({ label, index: pool[key].index, node_binary: pool[key].node_binary }));

const otherBranches = [];
for (const object of [pool.root, pool.amm]) {
  const key = previousTransaction(object.node_binary);
  const tx = await connection.call("tx", { transaction: key, binary: true });
  assert(tx.validated === true && tx.ledger_index < header.index, `${key} is in a ledger before ${header.index}: give --objects`);
  const other = await ledgerTree(connection, tx.ledger_index);
  const levels = branchLevels(other.tree, unhex(key)).levels;
  const entry = other.entries.find(({ tx_blob }) => hex(transactionId(tx_blob)) === key);
  assert(entry !== undefined && entry.tx_blob === tx.tx && entry.meta === tx.meta, "tx gives what the ledger holds");
  otherBranches.push({
    ledger_index: other.header.json.ledger_index,
    ledger_hash: other.header.hash,
    ledger_data: other.header.data,
    transaction_hash: other.header.json.transaction_hash,
    tx_blob: tx.tx,
    meta: tx.meta,
    key,
    levels,
  });
}
connection.close();

print({
  source: `Mainnet ledger ${header.index}: \`ledger\` with transactions, expand and binary (API v1); and one transaction's branch, each inner node on its path as its sixteen children's hashes, root first. Also the branches of the transactions that last changed the XRP/RLUSD pool's account root and AMM object, in other mainnet ledgers, from \`tx\` with binary, with their ledgers' headers in binary, and the pool's four ledger objects with their keys, from \`ledger_entry\` with binary as of ledger ${pool.root.ledger_index}. Captured ${captured} from ${MAINNET} by testdata/xrpl/capture-transaction-set.mjs.`,
  ledger_index: header.index,
  ledger_hash: header.hash,
  ledger_data: header.data,
  transaction_hash: header.json.transaction_hash,
  transactions: entries,
  branch: { key: hex(keys[at]), leaf_hash: branch.leafHash, levels: branch.levels },
  other_branches: otherBranches,
  objects,
});
