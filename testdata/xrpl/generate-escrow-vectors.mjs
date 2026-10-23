// Golden vectors for what an account several keys control is set up and
// paid out with (spec/areas/xrpl.md#the-transactions), from xrpl.js 3.1.0
// and the ripple-keypairs and @noble/curves it uses: secp256k1 keys and
// their signatures (spec/areas/xrpl.md#keys-and-signatures), with a
// signature of high S, which the ledger refuses; AccountSet,
// SetRegularKey, SignerListSet and Payments with a destination tag and no
// last ledger, single-signed by Ed25519 and secp256k1 wallets; and the
// same multi-signed, each signer alone, put together by xrpl.js's
// `multisign`.
//
// Run from the repository root, with xrpl@3.1.0 installed somewhere:
//
//   npm install --prefix "$TMP" xrpl@3.1.0
//   NODE_PATH="$TMP/node_modules" node testdata/xrpl/generate-escrow-vectors.mjs
//
// The output is deterministic (Ed25519 signs deterministically, and
// secp256k1 with RFC 6979): running it again must not change
// crates/dassie-xrpl/tests/data/escrow-vectors.json.

import { createRequire } from "node:module";
import { writeFileSync } from "node:fs";

const require = createRequire(`${process.env.NODE_PATH}/`);
const { Wallet, ECDSA, hashes, multisign } = require("xrpl");
const { encodeForSigning, encodeForMultisigning, decode } = require("ripple-binary-codec");
const keypairs = require("ripple-keypairs");
const { secp256k1 } = require("@noble/curves/secp256k1");

const hex = (text) => Buffer.from(text, "utf8").toString("hex").toUpperCase();
const entropy = (byte) => new Uint8Array(16).fill(byte);
const wallet = (byte, algorithm) => Wallet.fromEntropy(entropy(byte), { algorithm });

// A wallet as the vectors write it: its seed or secret, and its keys.
const describe = (w) => ({
  algorithm: w.publicKey.startsWith("ED") ? "ed25519" : "secp256k1",
  seed: w.seed,
  privateKey: w.privateKey,
  publicKey: w.publicKey,
  address: w.classicAddress,
});

// secp256k1 keys, each signing messages of several lengths. Its signature
// with S replaced by the group order less S verifies as plain ECDSA, but
// not as the ledger checks it.
const keys = [];
for (const byte of [1, 2, 3, 0x7f, 0xff]) {
  const w = wallet(byte, ECDSA.secp256k1);
  const signatures = ["", "00", hex("pay"), hex("x".repeat(300))].map((message) => {
    const signature = keypairs.sign(message, w.privateKey);
    const parsed = secp256k1.Signature.fromDER(signature);
    const high = new secp256k1.Signature(parsed.r, secp256k1.CURVE.n - parsed.s).toDERHex().toUpperCase();
    return {
      message,
      signature,
      verifies: keypairs.verify(message, signature, w.publicKey),
      highS: high,
      highSVerifies: keypairs.verify(message, high, w.publicKey),
    };
  });
  keys.push({ ...describe(w), signatures });
}

const ed = [wallet(0x11, ECDSA.ed25519), wallet(0x12, ECDSA.ed25519)];
const secp = [
  wallet(0x21, ECDSA.secp256k1),
  wallet(0x22, ECDSA.secp256k1),
  wallet(0x23, ECDSA.secp256k1),
];
const escrow = wallet(0x31, ECDSA.ed25519);
const member = wallet(0x32, ECDSA.secp256k1);

const signedVector = (w, tx) => {
  const signed = w.sign(tx);
  return {
    wallet: describe(w),
    tx,
    signingPayload: encodeForSigning({ ...tx, SigningPubKey: w.publicKey }),
    txBlob: signed.tx_blob,
    hash: hashes.hashSignedTx(signed.tx_blob),
    decoded: decode(signed.tx_blob),
  };
};

const entries = [...ed, ...secp].map((w) => ({
  SignerEntry: { Account: w.classicAddress, SignerWeight: 1 },
}));

// Single-signed: the setup of an escrow, by its master key or by a
// secp256k1 key; and a deposit to it, with a tag.
const single = [
  [escrow, { TransactionType: "AccountSet", Account: escrow.classicAddress, SetFlag: 1, Fee: "12", Flags: 0, Sequence: 7, LastLedgerSequence: 40 }],
  [escrow, { TransactionType: "AccountSet", Account: escrow.classicAddress, SetFlag: 4, ClearFlag: 1, Fee: "12", Flags: 0, Sequence: 8 }],
  [member, { TransactionType: "AccountSet", Account: member.classicAddress, ClearFlag: 4, Fee: "10", Flags: 0, Sequence: 0xffff_ffff, NetworkID: 21_338 }],
  [escrow, { TransactionType: "SetRegularKey", Account: escrow.classicAddress, RegularKey: member.classicAddress, Fee: "12", Flags: 0, Sequence: 9 }],
  [member, { TransactionType: "SetRegularKey", Account: member.classicAddress, Fee: "15", Flags: 0, Sequence: 3, LastLedgerSequence: 4_000_020 }],
  [escrow, { TransactionType: "SignerListSet", Account: escrow.classicAddress, SignerQuorum: 4, SignerEntries: entries, Fee: "12", Flags: 0, Sequence: 10 }],
  [member, { TransactionType: "SignerListSet", Account: member.classicAddress, SignerQuorum: 0, Fee: "12", Flags: 0, Sequence: 11 }],
  [member, { TransactionType: "Payment", Account: member.classicAddress, Destination: escrow.classicAddress, DestinationTag: 4_294_967_295, Amount: "25000000", Fee: "12", Flags: 0, Sequence: 12, LastLedgerSequence: 60, Memos: [{ Memo: { MemoType: hex("t"), MemoData: hex("d") } }] }],
].map(([w, tx]) => signedVector(w, tx));

// Multi-signed by the escrow's signers, each alone: a payout Payment with
// a tag and no last ledger, and a SignerListSet that replaces the signers
// with others. The fee is the base fee for each signer, and once more.
const multiVector = (tx, signers) => {
  const signed = signers.map((w) => ({
    wallet: describe(w),
    multiSigningPayload: encodeForMultisigning(tx, w.classicAddress),
    txBlob: w.sign(tx, true).tx_blob,
  }));
  const blob = multisign(signed.map((s) => s.txBlob));
  return { tx, signers: signed, txBlob: blob, hash: hashes.hashSignedTx(blob), decoded: decode(blob) };
};

const replacement = [wallet(0x41, ECDSA.secp256k1), wallet(0x42, ECDSA.secp256k1), wallet(0x43, ECDSA.secp256k1)];
const multi = [
  multiVector(
    { TransactionType: "Payment", Account: escrow.classicAddress, Destination: member.classicAddress, DestinationTag: 17, Amount: "1000000", Fee: "50", Flags: 0, Sequence: 21, SigningPubKey: "" },
    [secp[2], ed[0], secp[0], ed[1]],
  ),
  multiVector(
    {
      TransactionType: "SignerListSet",
      Account: escrow.classicAddress,
      SignerQuorum: 2,
      SignerEntries: replacement.map((w) => ({ SignerEntry: { Account: w.classicAddress, SignerWeight: 1 } })),
      Fee: "40",
      Flags: 0,
      Sequence: 22,
      SigningPubKey: "",
    },
    [secp[0], secp[1], secp[2]],
  ),
];

writeFileSync(
  "crates/dassie-xrpl/tests/data/escrow-vectors.json",
  `${JSON.stringify({ source: "xrpl@3.1.0", keys, single, multi }, null, 2)}\n`,
);
