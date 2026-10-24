// Captures crates/dassie-xrpl/tests/data/ledger-headers.json
// (spec/designs/xrpl-validation.md#13-what-pins-it), read-only: on each
// network, a validated ledger, its two parents and the ledger a thousand
// before it, each from the `ledger` method as JSON and with binary, the
// 118-byte header without its hash.
//
// Usage, from the repository root, at the validated ledgers or at given
// ones:
//
//   node testdata/xrpl/capture-ledger-headers.mjs [--mainnet <index>] [--testnet <index>] \
//     > crates/dassie-xrpl/tests/data/ledger-headers.json
//
// XRPL_MAINNET and XRPL_TESTNET name other servers (capture-lib.mjs); an
// older ledger needs one that keeps its history.

import { assert, headerFields, ledgerHeader, ledgerOption, MAINNET, open, options, print, TESTNET } from "./capture-lib.mjs";

const OFFSETS = [0, 1, 2, 1000];

const given = options(["mainnet", "testnet"]);
const captured = new Date().toISOString();
const ledgers = [];
for (const [network, url] of [["mainnet", MAINNET], ["testnet", TESTNET]]) {
  const connection = await open(url);
  const newest = await ledgerHeader(connection, ledgerOption(given[network]));
  for (const offset of OFFSETS) {
    const header = offset === 0 ? newest : await ledgerHeader(connection, newest.index - offset);
    assert(header.index === newest.index - offset, "the ledger asked for");
    ledgers.push({ network, ledger_index: header.index, ledger_hash: header.hash, ledger_data: header.data, json: headerFields(header.json) });
  }
  connection.close();
}

print({
  source: `The ledger method, as JSON and with binary: true; ledger_data is the 118-byte header without its hash. Captured ${captured} from ${MAINNET} and ${TESTNET} (rippled API v1) by testdata/xrpl/capture-ledger-headers.mjs.`,
  ledgers,
});
