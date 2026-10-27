// Golden vectors for `dassie-identity` (spec/areas/identity.md#derivation-chain),
// computed apart from Dassie's code, so that the node, the wallet's WASM
// build and this generator must agree. No library implements the phrase's
// Reed–Solomon code (spec/decisions/phrase-format.md), so it is written out
// below, unlike the Rust code: its field arithmetic by shift and add rather
// than by tables, its bits by BigInt, its division as long division. The
// rest comes from libraries of their own: the BIP-39 English wordlist from
// @scure/bip39, @noble/hashes for the master seed's HKDF, the HMAC tree of
// seed paths and the node ID's digest, @scure/base for its base32,
// @noble/curves for Ed25519 and X25519, @scure/bip32 for a hub's payout key,
// and xrpl.js for the XRP Ledger wallets.
//
// It writes, into crates/dassie-identity/tests/data/:
//
// - chain.json: phrases of both realms, each secret in each, from the
//   secret to the phrase, master seed, node key, public key, node ID, login
//   secret, and the entropy and wallet of the node's first XRP Ledger
//   account on the testnet and on mainnet; and readings: phrases as typed,
//   damaged, or not phrases at all, with what reading them gives;
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
const { entropyToMnemonic } = require("@scure/bip39");
const { wordlist } = require("@scure/bip39/wordlists/english.js");
const { HDKey } = require("@scure/bip32");
const { base32nopad } = require("@scure/base");
const { sha256 } = require("@noble/hashes/sha2.js");
const { hmac } = require("@noble/hashes/hmac.js");
const { hkdf } = require("@noble/hashes/hkdf.js");
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
// and the key, hashed; the first 24 bytes of the digest in lowercase
// base32, without padding.
const FINGERPRINT_OID = Uint8Array.from([
  0x0d, 0x2b, 0x06, 0x01, 0x04, 0x01, 0x83, 0xa8, 0x78, 0x05, 0x8a, 0x77, 0x02, 0x01,
]);
const hashInput = (publicKey) => Uint8Array.from([...FINGERPRINT_OID, 0x20, ...publicKey]);
const nodeId = (publicKey) => {
  const digest = sha256(hashInput(publicKey)).subarray(0, 24);
  return base32nopad.encode(digest).toLowerCase();
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

// GF(2^11) with x^11 + x^2 + 1 and alpha = x, multiplied by shift and add.
const gfMul = (a, b) => {
  let product = 0;
  for (; b > 0; b >>= 1, a <<= 1) {
    if (a & 0x800) a ^= 0x805;
    if (b & 1) product ^= a;
  }
  return product;
};
const gfPow = (power) => {
  let element = 1;
  for (let i = 0; i < power; i++) element = gfMul(element, 2);
  return element;
};

// g(x) = (x - a)(x - a^2)...(x - a^6), highest coefficient first.
const RS_GENERATOR = [1, 2, 3, 4, 5, 6].reduce(
  (g, root) => [...g, 0].map((c, i) => c ^ (i > 0 ? gfMul(g[i - 1], gfPow(root)) : 0)),
  [1],
);

// The check words: the data words shifted up by six places, divided by
// g(x), by long division over the dividend's coefficients.
const checkWords = (data) => {
  const dividend = [...data, 0, 0, 0, 0, 0, 0];
  for (let i = 0; i < data.length; i++) {
    const factor = dividend[i];
    if (factor === 0) continue;
    RS_GENERATOR.forEach((c, j) => (dividend[i + j] ^= gfMul(factor, c)));
  }
  return dividend.slice(data.length);
};

// The words of a kind, an hour and a 16-byte secret: 154 bits, most
// significant first, in 14 words of 11 bits, then the six check words.
const KINDS = { test: 1, live: 2 };
const EPOCH_MS = Date.UTC(2026, 0, 1);
const MAX_HOURS = 2 ** 20 - 1;
const phraseWords = (kind, created, secret) => {
  const bits = (BigInt(kind) << 148n) | (BigInt(created) << 128n) | BigInt(`0x${hex(secret)}`);
  const data = [];
  for (let i = 13; i >= 0; i--) data.push(Number((bits >> BigInt(11 * i)) & 0x7ffn));
  return [...data, ...checkWords(data)];
};
const text = (words) => words.map((index) => wordlist[index]).join(" ");

// The master seed: HKDF-SHA256 of the secret, salt "dassie phrase", the
// realm as info.
const masterSeedOf = (secret, realm) =>
  hkdf(sha256, secret, utf8("dassie phrase"), utf8(realm), 64);

// 32 secrets, each in both realms, made at hours across the field.
const chain = Array.from({ length: 64 }, (_, index) => {
  const realm = index % 2 === 0 ? "test" : "live";
  const secret = sha256(utf8(`dassie identity vector ${index >> 1}`)).subarray(0, 16);
  const digest = sha256(utf8(`dassie identity hour ${index}`));
  const created =
    index < 2 ? [0, MAX_HOURS][index] : ((digest[0] << 16) | (digest[1] << 8) | digest[2]) & MAX_HOURS;
  const masterSeed = masterSeedOf(secret, realm);
  const nodeKey = fold(masterSeed, ["node"]);
  const publicKey = ed25519.getPublicKey(nodeKey);
  const entry = {
    index,
    realm,
    kind: KINDS[realm],
    created,
    createdAt: new Date(EPOCH_MS + created * 3_600_000).toISOString(),
    secret: hex(secret),
    phrase: text(phraseWords(KINDS[realm], created, secret)),
    masterSeed: hex(masterSeed),
    nodeKey: hex(nodeKey),
    publicKey: hex(publicKey),
    nodeId: nodeId(publicKey),
    loginSecret: hex(derive(nodeKey, "node/login")),
  };
  for (const [name, path] of XRPL_ACCOUNTS) {
    const secret = derive(nodeKey, path);
    entry[`${name}Entropy`] = hex(secret);
    entry[`${name}Wallet`] = xrplWallet(secret);
  }
  return entry;
});

// Readings: what a person might type, and what reading it, expecting the
// realm given, must give. Damage within the bound (at most two wrong words,
// and 2 x wrong + unreadable at most five) restores the phrase, with the
// positions it changed; one step past it is refused, since the code's
// distance of seven leaves no other phrase within the bound.
const pick = (seed, count, from) => {
  const rank = (value) => hex(sha256(utf8(`${seed} ${value}`)));
  return [...from]
    .sort((a, b) => rank(a).localeCompare(rank(b)))
    .slice(0, count)
    .sort((a, b) => a - b);
};
const POSITIONS = Array.from({ length: 20 }, (_, i) => i);
const damage = (entry, seed, wrong, unreadable, mark) => {
  const words = entry.phrase.split(" ");
  const wrongAt = pick(`${seed} wrong`, wrong, POSITIONS);
  const rest = POSITIONS.filter((p) => !wrongAt.includes(p));
  const unreadableAt = pick(`${seed} unreadable`, unreadable, rest);
  for (const position of wrongAt) {
    const index = wordlist.indexOf(words[position]);
    words[position] = wordlist[(index + 1 + sha256(utf8(`${seed} ${position}`))[0]) % 2048];
  }
  for (const position of unreadableAt) {
    if (mark === "?") words[position] = "?";
    if (mark === "unknown") words[position] = "xyzzy";
  }
  return {
    input: words.join(" "),
    unreadable: mark === "marked" ? unreadableAt : [],
    changed: [...wrongAt, ...unreadableAt].sort((a, b) => a - b),
  };
};
const readings = [];
const at = (index) => chain[index];
// As written, in capitals, by the first four letters, with sloppy spaces.
readings.push({ input: at(0).phrase, unreadable: [], realm: "test", phrase: at(0).phrase, changed: [] });
readings.push({
  input: `  ${at(1).phrase.toUpperCase()}\n`,
  unreadable: [],
  realm: "live",
  phrase: at(1).phrase,
  changed: [],
});
readings.push({
  input: at(2).phrase.split(" ").map((word) => word.slice(0, 4)).join("\t"),
  unreadable: [],
  realm: "test",
  phrase: at(2).phrase,
  changed: [],
});
// Damage within the bound, unreadable words written as "?", as an unknown
// word, or marked by position.
const WITHIN = [[1, 0], [2, 0], [0, 1], [0, 5], [1, 3], [2, 1], [1, 1], [0, 3]];
const MARKS = ["?", "unknown", "marked"];
WITHIN.forEach(([wrong, unreadable], i) => {
  MARKS.forEach((mark, j) => {
    const entry = at(3 + i * MARKS.length + j);
    const reading = damage(entry, `within ${i} ${j}`, wrong, unreadable, mark);
    readings.push({ ...reading, realm: entry.realm, phrase: entry.phrase });
  });
});
// One step past the bound.
const BEYOND = [[3, 0], [2, 2], [1, 4], [0, 6], [3, 1]];
BEYOND.forEach(([wrong, unreadable], i) => {
  const entry = at(40 + i);
  const reading = damage(entry, `beyond ${i}`, wrong, unreadable, i % 2 === 0 ? "?" : "marked");
  readings.push({
    input: reading.input,
    unreadable: reading.unreadable,
    realm: entry.realm,
    error: "tooDamaged",
  });
});
// BIP-39 phrases, and counts that are no phrase.
for (const bytes of [16, 32]) {
  const entropy = sha256(utf8(`dassie bip39 ${bytes}`)).subarray(0, bytes);
  readings.push({ input: entropyToMnemonic(entropy, wordlist), unreadable: [], realm: "test", error: "bip39" });
}
const words = at(0).phrase.split(" ");
for (const [input, error] of [
  [words.slice(0, 19).join(" "), "wordCount"],
  [[...words, "abandon"].join(" "), "bip39"],
  [[...words, "abandon", "abandon"].join(" "), "wordCount"],
  ["", "wordCount"],
]) {
  readings.push({ input, unreadable: [], realm: "test", error });
}
// A phrase of the other realm, and kinds this version does not know.
readings.push({ input: at(1).phrase, unreadable: [], realm: "test", error: "otherRealm" });
readings.push({ input: at(0).phrase, unreadable: [], realm: "live", error: "otherRealm" });
const secret = Uint8Array.from(Buffer.from(at(0).secret, "hex"));
for (const [kind, error] of [[0, "invalidKind"], [3, "newerKind"], [63, "newerKind"]]) {
  readings.push({ input: text(phraseWords(kind, 0, secret)), unreadable: [], realm: "test", error });
}
// The kind is read after correction: a damaged phrase of the other realm
// is refused as that, not shown corrected.
readings.push({
  input: damage(at(5), "other realm", 1, 1, "?").input,
  unreadable: [],
  realm: "test",
  error: "otherRealm",
});

write("chain.json", { chain, readings });

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
  { name: "routing", path: "node/routing" },
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
