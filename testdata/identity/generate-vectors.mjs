// Golden vectors for `dassie-identity` (spec/areas/identity.md#derivation-chain),
// computed with libraries of their own rather than with Dassie's code, so
// that the node, the wallet's WASM build and these libraries must agree:
// @scure/bip39 for the phrase and the master seed, @noble/hashes for the
// HMAC tree of seed paths and the node ID's digest, @noble/curves for
// Ed25519 and X25519, @scure/bip32 for a hub's payout key, and xrpl.js for
// the XRP Ledger wallets.
//
// It writes, into crates/dassie-identity/tests/data/:
//
// - chain.json: phrases of every length, each from its entropy to its
//   master seed, node key, public key, node ID, login secret, development
//   session token, and the entropy and wallet of the node's first XRP
//   Ledger account on the testnet and on mainnet;
// - keys.json: two node keys with a signature and their X25519 keys, an
//   X25519 shared secret, and the node ID's hash input;
// - paths.json: every seed path below the node key, with its text and its
//   secret, and for a hub's keys what the hub's rules name of them.
//
// Run from the repository root, with the libraries installed somewhere:
//
//   npm install --prefix "$TMP" @scure/bip39@2.4.0 @scure/bip32@2.4.0 \
//     @scure/base@2.4.0 @noble/hashes@2.4.0 @noble/curves@2.4.0 xrpl@3.1.0
//   NODE_PATH="$TMP/node_modules" node testdata/identity/generate-vectors.mjs
//
// The output is deterministic: running it again must not change the files.

import { createRequire } from "node:module";
import { writeFileSync } from "node:fs";

const require = createRequire(`${process.env.NODE_PATH}/`);
const { entropyToMnemonic, mnemonicToSeedSync } = require("@scure/bip39");
const { wordlist } = require("@scure/bip39/wordlists/english.js");
const { HDKey } = require("@scure/bip32");
const { sha256 } = require("@noble/hashes/sha2.js");
const { hmac } = require("@noble/hashes/hmac.js");
const { ed25519, x25519 } = require("@noble/curves/ed25519.js");
const { Wallet, encodeSeed } = require("xrpl");

const OUT = "crates/dassie-identity/tests/data";
const GENERATOR = "testdata/identity/generate-vectors.mjs";

const hex = (bytes) => Buffer.from(bytes).toString("hex");
const utf8 = (text) => new TextEncoder().encode(text);
const fill = (byte) => new Uint8Array(32).fill(byte);

const write = (name, vectors) => {
  const document = { generator: GENERATOR, ...vectors };
  writeFileSync(`${OUT}/${name}`, `${JSON.stringify(document, undefined, 2)}\n`);
};

// The seed-path tree: each segment is the HMAC-SHA256 of its name under
// the key of the path before it.
const fold = (key, segments) =>
  segments.reduce((current, segment) => hmac(sha256, current, utf8(segment)), key);
const derive = (nodeKey, path) => {
  const [first, ...rest] = path.split("/");
  if (first !== "node") throw new Error(`not below the node key: ${path}`);
  return fold(nodeKey, rest);
};

// The node ID: the OER of the fingerprint OID 1.3.6.1.4.1.54392.5.1399.2.1
// and the key, hashed; the first 24 bytes of the digest in base64url, with
// trailing `A`s dropped, after a `d`.
const FINGERPRINT_OID = Uint8Array.from([
  0x0d, 0x2b, 0x06, 0x01, 0x04, 0x01, 0x83, 0xa8, 0x78, 0x05, 0x8a, 0x77, 0x02, 0x01,
]);
const hashInput = (publicKey) => Uint8Array.from([...FINGERPRINT_OID, 0x20, ...publicKey]);
const nodeId = (publicKey) => {
  const digest = sha256(hashInput(publicKey)).subarray(0, 24);
  return `d${Buffer.from(digest).toString("base64url").replace(/A+$/, "")}`;
};

// An XRP Ledger wallet of 32 bytes of entropy: an Ed25519 family seed of
// its first 16.
const xrplWallet = (entropy) => {
  const familySeed = encodeSeed(entropy.subarray(0, 16), "ed25519");
  const wallet = Wallet.fromSeed(familySeed);
  return { familySeed, address: wallet.classicAddress, publicKey: wallet.publicKey };
};

// The node's first account on each network, by its network ID.
const XRPL_ACCOUNTS = [
  ["xrplTestnet", "node/xrpl/1/account/0"],
  ["xrplMainnet", "node/xrpl/0/account/0"],
];

// Entropy of every length a phrase has (12 to 24 words), from a counter.
const ENTROPY_BYTES = [16, 20, 24, 28, 32];
const chain = Array.from({ length: 64 }, (_, index) => {
  const length = ENTROPY_BYTES[index % ENTROPY_BYTES.length];
  const entropy = sha256(utf8(`dassie identity vector ${index}`)).subarray(0, length);
  const mnemonic = entropyToMnemonic(entropy, wordlist);
  const masterSeed = mnemonicToSeedSync(mnemonic);
  const nodeKey = fold(masterSeed, ["node"]);
  const publicKey = ed25519.getPublicKey(nodeKey);
  const entry = {
    index,
    entropy: hex(entropy),
    mnemonic,
    masterSeed: hex(masterSeed),
    nodeKey: hex(nodeKey),
    publicKey: hex(publicKey),
    nodeId: nodeId(publicKey),
    loginSecret: hex(derive(nodeKey, "node/login")),
    devSessionToken: hex(fold(masterSeed, ["dev", "session"])),
  };
  for (const [name, path] of XRPL_ACCOUNTS) {
    const secret = derive(nodeKey, path);
    entry[`${name}Entropy`] = hex(secret);
    entry[`${name}Wallet`] = xrplWallet(secret);
  }
  return entry;
});
write("chain.json", { chain });

const message = utf8("dassie test message");
const keys = [fill(1), fill(3)].map((seed) => {
  const publicKey = ed25519.getPublicKey(seed);
  return {
    seed: hex(seed),
    publicKey: hex(publicKey),
    nodeId: nodeId(publicKey),
    x25519Secret: hex(ed25519.utils.toMontgomerySecret(seed)),
    x25519Public: hex(ed25519.utils.toMontgomery(publicKey)),
    message: hex(message),
    signature: hex(ed25519.sign(message, seed)),
  };
});
const sessionPrivate = fill(2);
const peerPublic = ed25519.getPublicKey(fill(3));
write("keys.json", {
  keys,
  session: {
    privateKey: hex(sessionPrivate),
    publicKey: hex(x25519.getPublicKey(sessionPrivate)),
    peerPublicKey: hex(peerPublic),
    sharedSecret: hex(x25519.getSharedSecret(sessionPrivate, ed25519.utils.toMontgomery(peerPublic))),
  },
  nodeIdHashInput: {
    oid: "1.3.6.1.4.1.54392.5.1399.2.1",
    publicKey: hex(ed25519.getPublicKey(fill(1))),
    encoded: hex(FINGERPRINT_OID),
  },
});

// Every path below the node key, by the name of its function in
// `dassie-identity` (src/paths.rs) and its arguments.
const LEDGERS = [
  "stub+usd",
  "xrpl-testnet-xrp+xrp",
  "xrpl-xrp+xrp",
  "xrpl-testnet-rlusd+usd",
  "xrpl-rlusd+usd",
  "xrpl-testnet-dsusd+usd",
  "hub-xrpl-testnet-xrp+xrp",
  "hub-xrpl-testnet-dsusd+usd",
];
const QUIC = [
  ["udp", "stateless-reset", "connection-ids"],
  ["tunnel", "tunnel-stateless-reset", "tunnel-connection-ids"],
];
const HUB_KEYS = ["consensus", "escrow", "payout", "escrow-master"];
const NONCE = hex(fill(2));
const HUB = hex(fill(2));

const paths = [
  { name: "login", path: "node/login" },
  { name: "openPaymentsClient", path: "node/open-payments-client" },
  ...LEDGERS.map((ledger) => ({
    name: "streamReceiver",
    ledger,
    path: `node/stream-server/${ledger}`,
  })),
  ...QUIC.flatMap(([endpoint, reset, ids]) => [
    { name: "quicStatelessReset", endpoint, path: `node/quic/${reset}` },
    { name: "quicConnectionIds", endpoint, path: `node/quic/${ids}` },
  ]),
  ...[1, 0].flatMap((network) =>
    HUB_KEYS.map((key) => ({
      name: "hub",
      network,
      nonce: NONCE,
      key,
      path: `node/hub/${network}/nonce/${NONCE}/${key}`,
    }))
  ),
  { name: "hubMember", hub: HUB, path: `node/hub/member/${HUB}/account` },
  ...[[1, 0], [0, 0], [1, 1], [21338, 65535]].map(([network, index]) => ({
    name: "xrplAccount",
    network,
    index,
    path: `node/xrpl/${network}/account/${index}`,
  })),
];

// What a hub's rules and its escrow name of a key: an Ed25519 key for
// consensus and a member's account, an XRP Ledger account for the escrow's
// signer and its master, and a BIP 32 master public key for payouts.
const named = (entry, secret) => {
  switch (entry.key ?? (entry.name === "hubMember" ? "member" : undefined)) {
    case "consensus":
    case "member":
      return { publicKey: hex(ed25519.getPublicKey(secret)) };
    case "escrow":
    case "escrow-master":
      return { wallet: xrplWallet(secret) };
    case "payout": {
      const master = HDKey.fromMasterSeed(secret);
      return { payoutPublicKey: hex(master.publicKey), payoutChainCode: hex(master.chainCode) };
    }
    default:
      return {};
  }
};

const nodeKey = fill(1);
write("paths.json", {
  nodeKey: hex(nodeKey),
  paths: paths.map((entry) => {
    const secret = derive(nodeKey, entry.path);
    return { ...entry, secret: hex(secret), ...named(entry, secret) };
  }),
});
