// Captures crates/dassie-xrpl/tests/data/account-delete.json
// (spec/designs/xrpl-validation.md#13-what-pins-it), read-only: a mainnet
// `AccountDelete` from `tx`, as JSON and in binary. Its metadata deletes
// the sender's account root, whose final balance is zero and whose balance
// before is in its previous fields, and pays what was left to the
// destination's root.
//
// Usage, from the repository root:
//
//   node testdata/xrpl/capture-account-delete.mjs [--transaction <hash>] \
//     > crates/dassie-xrpl/tests/data/account-delete.json
//
// XRPL_MAINNET names another server (capture-lib.mjs); an older ledger
// needs one that keeps its history.

import { assert, MAINNET, open, options, print } from "./capture-lib.mjs";

const TRANSACTION = "0D3C69E78A361ACC654C315E15997301D1F334A7D28BF67D9B3D0289A934427D";

const given = options(["transaction"]);
const hash = given.transaction ?? TRANSACTION;
const captured = new Date().toISOString();
const connection = await open(MAINNET);
const json = await connection.call("tx", { transaction: hash });
const binary = await connection.call("tx", { transaction: hash, binary: true });
connection.close();

assert(json.validated === true && binary.validated === true, `${hash} is validated`);
assert(json.TransactionType === "AccountDelete", `${hash} is an AccountDelete`);
assert(json.ledger_index === binary.ledger_index, "both answers name one ledger");
const deleted = json.meta.AffectedNodes.find(
  (node) => node.DeletedNode?.LedgerEntryType === "AccountRoot",
)?.DeletedNode;
assert(deleted !== undefined, "the metadata deletes an account root");
assert(deleted.FinalFields.Balance === "0", "the deleted root's final balance is zero");
assert(deleted.PreviousFields?.Balance !== undefined, "the deleted root's balance before is in its previous fields");

print({
  source: `Mainnet transaction ${hash} in ledger ${json.ledger_index}, an AccountDelete: \`tx\` as JSON and with binary (API v1). Captured ${captured} from ${MAINNET} by testdata/xrpl/capture-account-delete.mjs.`,
  hash,
  ledger_index: json.ledger_index,
  json,
  tx_blob: binary.tx,
  meta: binary.meta,
});
