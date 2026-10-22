// Golden vectors for `dassie-xrpl` from xrpl.js 3.1.0, the library the
// original's XRPL module used (spec/areas/xrpl.md#the-payment-it-signs):
// settlement Payments as the original builds them, signed by the wallets of
// crates/dassie-identity/tests/data/chain.json, with what xrpl.js signs,
// the blob it submits and the hash it reports. Then the token wire format
// (spec/areas/xrpl.md#tokens): token amounts on their own, at the edges of
// what the ledger can express, and token Payments and TrustSets as xrpl.js
// signs them, with RLUSD of its testnet issuer. Last, cross-currency
// Payments with `SendMax` (spec/areas/xrpl.md#tokens): one asset delivered,
// at most `SendMax` of the other spent, through the ledger's default paths.
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
const { Amount } = require("ripple-binary-codec/dist/types");

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

// RLUSD on the testnet: its issuer, and its currency code, which is
// longer than three letters and so written in hex.
const RLUSD = "524C555344000000000000000000000000000000";
const RLUSD_ISSUER = "rQhWct2fv4Vc4KRjRgMrxa8xPN9Zx9iLKV";

// Token values at the edges: the smallest and largest the ledger can
// express, sixteen significant digits, a millionth (the ledger unit of
// `xrpl-testnet-rlusd+usd`), zero, negative values, and the forms rippled
// writes (a decimal, or a mantissa and an exponent).
const values = [
  "0",
  "1",
  "-1",
  "0.000001",
  "1e-81",
  "-1e-81",
  "9999999999999999e80",
  "-9999999999999999e80",
  "1234567890.123456",
  "1234567890123456",
  "0.0000000000000001234567890123456",
  "1000000000000000e-3",
  "1000000000000000e1",
  "99999999.99999999",
  "12345678901234560000",
];
const amounts = [];
for (const [index, value] of values.entries()) {
  const amount = {
    currency: index % 3 === 2 ? "USD" : RLUSD,
    issuer: index % 3 === 1 ? chain[0].xrplTestnetWallet.address : RLUSD_ISSUER,
    value,
  };
  const encoded = Amount.from(amount);
  amounts.push({ amount, hex: encoded.toHex(), decoded: encoded.toJSON() });
}

// Token Payments as the scheme sends them: a token Amount, no paths, no
// SendMax, no flags; and TrustSets with rippling disabled on our side.
const tokenShapes = [
  { value: "0.000001", fee: "12", sequence: 7, last: 27 },
  { value: "1234567890.123456", fee: "15", sequence: 4_000_000, last: 4_000_020 },
  { value: "1e-81", fee: "10", sequence: 1, last: 21, network: 21_338 },
  { value: "9999999999999999e80", fee: "1000", sequence: 0xffff_ffff, last: 0xffff_ffff },
];
const trustShapes = [
  { value: "9999999999999999e80", flags: 0x0002_0000, fee: "12", sequence: 3, last: 23 },
  { value: "0", flags: 0x0002_0000, fee: "10", sequence: 9, last: 29 },
  { value: "1000000", flags: 0, fee: "15", sequence: 77, last: 120, network: 21_338 },
  { value: "1e-81", flags: 0x0002_0000, fee: "12", sequence: 0xffff_ffff, last: 0xffff_ffff, currency: "USD" },
];

const signedVector = (from, tx, wallet) => {
  const signed = wallet.sign(tx);
  return {
    familySeed: from.xrplTestnetWallet.familySeed,
    nodeId: from.nodeId,
    tx,
    signingPayload: encodeForSigning({ ...tx, SigningPubKey: wallet.publicKey }),
    txBlob: signed.tx_blob,
    hash: hashes.hashSignedTx(signed.tx_blob),
    decoded: decode(signed.tx_blob),
  };
};
const network = (shape) => (shape.network === undefined ? {} : { NetworkID: shape.network });

const tokenPayments = [];
for (const [index, shape] of tokenShapes.entries()) {
  const from = chain[20 + index * 3];
  const to = chain[20 + index * 3 + 1];
  const wallet = Wallet.fromSeed(from.xrplTestnetWallet.familySeed);
  const tx = {
    TransactionType: "Payment",
    Account: wallet.address,
    Amount: { currency: RLUSD, issuer: RLUSD_ISSUER, value: shape.value },
    Destination: to.xrplTestnetWallet.address,
    Fee: shape.fee,
    Flags: 0,
    Sequence: shape.sequence,
    LastLedgerSequence: shape.last,
    Memos: [{ Memo: { MemoType: MEMO_TYPE, MemoData: hex(from.nodeId) } }],
    ...network(shape),
  };
  tokenPayments.push(signedVector(from, tx, wallet));
}

const trustSets = [];
for (const [index, shape] of trustShapes.entries()) {
  const from = chain[40 + index * 3];
  const wallet = Wallet.fromSeed(from.xrplTestnetWallet.familySeed);
  const tx = {
    TransactionType: "TrustSet",
    Account: wallet.address,
    LimitAmount: { currency: shape.currency ?? RLUSD, issuer: RLUSD_ISSUER, value: shape.value },
    Fee: shape.fee,
    Flags: shape.flags,
    Sequence: shape.sequence,
    LastLedgerSequence: shape.last,
    ...network(shape),
  };
  trustSets.push(signedVector(from, tx, wallet));
}

// Payments with `SendMax` in the other asset, XRP to a token and a token to
// XRP, as a swap through an AMM pool is sent: no paths, not partial, with
// and without a memo, at every width the codec has.
const sendMaxShapes = [
  { amount: { value: "25" }, sendMax: "30000000", fee: "12", sequence: 5, last: 25 },
  { amount: "1000000", sendMax: { value: "0.75" }, fee: "15", sequence: 4_000_000, last: 4_000_020, memo: true },
  { amount: { value: "1e-81", currency: "USD" }, sendMax: "100000000000", fee: "1000", sequence: 0xffff_ffff, last: 0xffff_ffff, network: 21_338, memo: true },
  { amount: "1", sendMax: { value: "9999999999999999e80" }, fee: "10", sequence: 1, last: 21 },
];
const sendMaxAmount = (amount) =>
  typeof amount === "string"
    ? amount
    : { currency: amount.currency ?? RLUSD, issuer: RLUSD_ISSUER, value: amount.value };

const sendMaxPayments = [];
for (const [index, shape] of sendMaxShapes.entries()) {
  const from = chain[52 + index * 3];
  const to = chain[52 + index * 3 + 1];
  const wallet = Wallet.fromSeed(from.xrplTestnetWallet.familySeed);
  const tx = {
    TransactionType: "Payment",
    Account: wallet.address,
    Amount: sendMaxAmount(shape.amount),
    SendMax: sendMaxAmount(shape.sendMax),
    Destination: to.xrplTestnetWallet.address,
    Fee: shape.fee,
    Flags: 0,
    Sequence: shape.sequence,
    LastLedgerSequence: shape.last,
    ...(shape.memo ? { Memos: [{ Memo: { MemoType: MEMO_TYPE, MemoData: hex(from.nodeId) } }] } : {}),
    ...network(shape),
  };
  sendMaxPayments.push(signedVector(from, tx, wallet));
}

writeFileSync(
  "crates/dassie-xrpl/tests/data/vectors.json",
  `${JSON.stringify(
    {
      source: "xrpl@3.1.0",
      memoType: MEMO_TYPE,
      payments,
      amounts,
      tokenPayments,
      trustSets,
      sendMaxPayments,
    },
    null,
    2,
  )}\n`,
);
