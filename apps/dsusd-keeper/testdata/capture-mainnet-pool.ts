// Captures testdata/mainnet-pool.json, which src/price_test.ts reads:
// mainnet's answers for the price, whole, as the keeper reads them
// (`readMainnetPool` in src/ledger.ts), from its default server or the one
// DSUSD_KEEPER_PRICE_SERVER names.
//
//   node apps/dsusd-keeper/testdata/capture-mainnet-pool.ts \
//     > apps/dsusd-keeper/testdata/mainnet-pool.json

import process from "node:process";
import { MAINNET_SERVER } from "../src/config.ts";
import { readMainnetPool } from "../src/ledger.ts";

const server = process.env["DSUSD_KEEPER_PRICE_SERVER"] ?? MAINNET_SERVER;
const answers = await readMainnetPool(server);
if (!answers.ok) throw new Error(answers.error);
const source = `Answers of ${server} through xrpl.js, whole: ledger of the ` +
  `newest validated ledger, and amm_info of the XRP/RLUSD pool at its ` +
  `hash. Captured ${new Date().toISOString()} by ` +
  `apps/dsusd-keeper/testdata/capture-mainnet-pool.ts.`;
console.log(JSON.stringify({ source, ...answers.value }, null, 2));
