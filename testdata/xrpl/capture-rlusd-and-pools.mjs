// Captures crates/dassie-xrpl/tests/data/rlusd-and-pools.json
// (spec/designs/xrpl-validation.md#13-what-pins-it), read-only, each
// answer whole at API version 1: on mainnet, RLUSD's issuer by
// `account_info` and `gateway_balances`, and the XRP/RLUSD pool by
// `amm_info` and its account root, its RLUSD trust line and its AMM object
// by `ledger_entry` with binary; on the testnet, `amm_info` of the XRP
// pools of DSUSD and of RLUSD's testnet issuer.
//
// Usage, from the repository root, at the validated ledgers or at given
// ones:
//
//   node testdata/xrpl/capture-rlusd-and-pools.mjs [--mainnet <index>] [--testnet <index>] \
//     > crates/dassie-xrpl/tests/data/rlusd-and-pools.json
//
// XRPL_MAINNET and XRPL_TESTNET name other servers (capture-lib.mjs); an
// older ledger needs one that keeps its history.

import { assert, ledgerHeader, ledgerOption, MAINNET, open, options, poolObjects, print, RLUSD, TESTNET, XRP } from "./capture-lib.mjs";

const TESTNET_POOLS = {
  "testnet_DSUSD_amm_info": { currency: "4453555344000000000000000000000000000000", issuer: "rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M" },
  "testnet_RLUSD-testnet_amm_info": { currency: RLUSD.currency, issuer: "rQhWct2fv4Vc4KRjRgMrxa8xPN9Zx9iLKV" },
};

const given = options(["mainnet", "testnet"]);
const captured = new Date().toISOString();

const mainnet = await open(MAINNET);
const at = { ledger_index: (await ledgerHeader(mainnet, ledgerOption(given.mainnet))).index };
const issuer = { account: RLUSD.issuer, ...at };
const answers = {
  issuer_account_info: await mainnet.call("account_info", issuer),
  gateway_balances: await mainnet.call("gateway_balances", issuer),
};
const pool = await poolObjects(mainnet, at.ledger_index);
mainnet.close();
Object.assign(answers, { amm_info: pool.info, amm_account_root: pool.root, amm_rlusd_line: pool.line, amm_object: pool.amm });

const testnet = await open(TESTNET);
const testnetAt = { ledger_index: (await ledgerHeader(testnet, ledgerOption(given.testnet))).index };
for (const [name, asset2] of Object.entries(TESTNET_POOLS)) {
  answers[name] = await testnet.call("amm_info", { asset: XRP, asset2, ...testnetAt });
  assert(answers[name].amm.amount2.issuer === asset2.issuer, `${name} pairs XRP with its token`);
}
testnet.close();

print({
  source: `Answers of mainnet's ledger ${at.ledger_index} and the testnet's ledger ${testnetAt.ledger_index}, whole, from ${MAINNET} and ${TESTNET} (rippled API v1): RLUSD's issuer by account_info and gateway_balances, the XRP/RLUSD pool by amm_info and its objects by ledger_entry with binary, and the testnet's XRP pools of DSUSD and of RLUSD by amm_info. Captured ${captured} by testdata/xrpl/capture-rlusd-and-pools.mjs.`,
  ...answers,
});
