// Golden vectors for `dassie-xrpl` from xrpl.js 3.1.0, the library the
// original's XRPL module used (spec 5 §6.3): settlement Payments as the
// original builds them, signed by the wallets of
// crates/dassie-identity/tests/data/chain.json, with what xrpl.js signs,
// the blob it submits and the hash it reports.
//
// Run from the repository root, with xrpl@3.1.0 installed somewhere:
//
//   npm install --prefix "$TMP" xrpl@3.1.0
//   NODE_PATH="$TMP/node_modules" node testdata/xrpl/generate-vectors.mjs
//
// The output is deterministic (Ed25519 signs deterministically): running it
// again must not change crates/dassie-xrpl/tests/data/vectors.json.

import { createRequire } from "node:module";
import { readFileSync, writeFileSync } from "node:fs";

const require = createRequire(`${process.env.NODE_PATH}/`);
const { Wallet, hashes } = require("xrpl");
const { encodeForSigning, decode } = require("ripple-binary-codec");

const chain = JSON.parse(
  readFileSync("crates/dassie-identity/tests/data/chain.json", "utf8"),
).chain;

const hex = (text) => Buffer.from(text, "utf8").toString("hex").toUpperCase();
const MEMO_TYPE = hex("https://dassie.land/settlement/xrp-testnet/v1");

// Amounts and fields that reach every width the codec has.
const shapes = [
  { amount: "1", fee: "10", sequence: 1, last: 21 },
  { amount: "999999", fee: "12", sequence: 4_000_000, last: 4_000_020 },
  { amount: "100000000000", fee: "1000", sequence: 0xffff_ffff, last: 0xffff_ffff },
  { amount: "123456789", fee: "15", sequence: 77, last: 120, network: 21_338 },
];

const payments = [];
for (const [index, shape] of shapes.entries()) {
  const from = chain[index * 3];
  const to = chain[index * 3 + 1];
  const wallet = Wallet.fromSeed(from.xrplTestnetWallet.familySeed);
  const tx = {
    TransactionType: "Payment",
    Account: wallet.address,
    Amount: shape.amount,
    Destination: to.xrplTestnetWallet.address,
    Fee: shape.fee,
    Flags: 0,
    Sequence: shape.sequence,
    LastLedgerSequence: shape.last,
    Memos: [{ Memo: { MemoType: MEMO_TYPE, MemoData: hex(from.nodeId) } }],
    ...(shape.network === undefined ? {} : { NetworkID: shape.network }),
  };
  const signed = wallet.sign(tx);
  payments.push({
    familySeed: from.xrplTestnetWallet.familySeed,
    nodeId: from.nodeId,
    tx,
    signingPayload: encodeForSigning({ ...tx, SigningPubKey: wallet.publicKey }),
    txBlob: signed.tx_blob,
    hash: hashes.hashSignedTx(signed.tx_blob),
    decoded: decode(signed.tx_blob),
  });
}

writeFileSync(
  "crates/dassie-xrpl/tests/data/vectors.json",
  `${JSON.stringify({ source: "xrpl@3.1.0", memoType: MEMO_TYPE, payments }, null, 2)}\n`,
);
