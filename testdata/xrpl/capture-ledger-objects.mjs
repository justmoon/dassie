// Captures crates/dassie-xrpl/tests/data/ledger-objects.json
// (spec/designs/xrpl-validation.md#13-what-pins-it), read-only: a mainnet
// ledger's header from `ledger` with binary and, from `ledger_entry` with
// binary as of that ledger, its skip list of recent ledgers, the skip list
// that holds its newest flag ledger, and the fee settings.
//
// Usage, from the repository root, at the validated ledger or a given one:
//
//   node testdata/xrpl/capture-ledger-objects.mjs [--ledger <index>] \
//     > crates/dassie-xrpl/tests/data/ledger-objects.json
//
// XRPL_MAINNET names another server (capture-lib.mjs); an older ledger
// needs one that keeps its history.

import { assert, hex, ledgerHeader, ledgerOption, MAINNET, open, options, print, sha512Half } from "./capture-lib.mjs";

const RECENT = "B4979A36CDC7F3D3D5C31A4EAE2AC7D7209DDA877588B9AFC66799692AB0D66B";
const FEES = "4BC50C9B0D8515D3EAAE1E74B29A95804346C491EE1A95BF25E4AAB854A6A651";
const SKIP_SPACE = 0x0073;
const FLAG_INTERVAL = 256;

// The key of the skip list that holds flag ledger `sequence`: one for
// every 65536 ledgers.
function flagSkipKey(sequence) {
  const bytes = Buffer.alloc(6);
  bytes.writeUInt16BE(SKIP_SPACE, 0);
  bytes.writeUInt32BE(sequence >>> 16, 2);
  return hex(sha512Half(bytes));
}

const given = options(["ledger"]);
const captured = new Date().toISOString();
const connection = await open(MAINNET);
const header = await ledgerHeader(connection, ledgerOption(given.ledger));
const entry = async (label, index, extra = {}) => {
  const result = await connection.call("ledger_entry", { index, binary: true, ledger_index: header.index });
  assert(result.index === index && typeof result.node_binary === "string", `${label} in binary`);
  return { label, ...extra, index, node_binary: result.node_binary };
};
const flagLedger = header.index - (header.index % FLAG_INTERVAL);
const objects = [
  await entry("LedgerHashes (recent)", RECENT),
  await entry("LedgerHashes (flag ledgers)", flagSkipKey(flagLedger), { flag_ledger: flagLedger }),
  await entry("FeeSettings", FEES),
];
connection.close();

print({
  source: `Mainnet ledger ${header.index} (ledger with binary) and, from ledger_entry with binary as of it, its skip list of recent ledgers, the skip list that holds its newest flag ledger, and the fee settings. Captured ${captured} from ${MAINNET} (rippled API v1) by testdata/xrpl/capture-ledger-objects.mjs.`,
  ledger_index: header.index,
  ledger_hash: header.hash,
  ledger_data: header.data,
  objects,
});
