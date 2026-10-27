// Golden vectors for `dassie-identity` (spec/areas/identity.md#derivation-chain),
// computed with libraries of their own rather than with Dassie's code, so
// that the node, the wallet's WASM build and these libraries must agree:
// phrase.mjs beside this file for the phrase format, written from its
// decision alone, with node:crypto's HKDF-SHA256 for the master seed;
// @scure/bip39 for the English wordlist and for BIP-39 phrases a reader
// refuses; @noble/hashes for the HMAC tree of seed paths and the node ID's
// digest, @scure/base for its base32, @noble/curves for Ed25519,
// @scure/bip32 for a hub's payout key, and xrpl.js for the XRP Ledger
// wallets.
//
// It writes, into crates/dassie-identity/tests/data/:
//
// - phrase.json: the phrase format on its own: phrases of both realms
//   made from a kind, a creation hour and a secret, with their data and
//   check words and master seed; the hours of clock readings; and phrases
//   as a reader reads them, with wrong, unreadable and unknown words, of
//   every length, kind and realm, and made more than a day after the
//   reader's clock;
// - chain.json: phrases of both realms, each from its secret to its
//   master seed, node key, public key, node ID, login secret, and the
//   entropy and wallet of the node's first XRP Ledger account on the
//   testnet and on mainnet;
// - keys.json: two node keys with a signature, and the node ID's hash
//   input;
// - paths.json: every seed path below the node key, with its text and its
//   secret, and for a hub's keys what the hub's rules name of them.
//
// Run from the repository root, with the libraries installed somewhere:
//
//   npm install --prefix "$TMP" @scure/bip39@2.4.0 @scure/bip32@2.4.0 \
//     @scure/base@2.4.0 @noble/hashes@2.4.0 @noble/curves@2.4.0 xrpl@3.1.0
//   NODE_PATH="$TMP/node_modules" node testdata/identity/generate-vectors.mjs
//
// Then regenerate testdata/xrpl's vectors, which sign with chain.json's
// wallets. The output is deterministic: running it again must not change
// the files.

import { createRequire } from "node:module";
import { writeFileSync } from "node:fs";
import * as phrase from "./phrase.mjs";

const require = createRequire(`${process.env.NODE_PATH}/`);
const { entropyToMnemonic } = require("@scure/bip39");
const { wordlist } = require("@scure/bip39/wordlists/english.js");
const { HDKey } = require("@scure/bip32");
const { base32nopad } = require("@scure/base");
const { sha256 } = require("@noble/hashes/sha2.js");
const { hmac } = require("@noble/hashes/hmac.js");
const { ed25519 } = require("@noble/curves/ed25519.js");
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

// Phrases of both realms, a secret and a creation hour each from a counter.
const words = (kind, created, secret) => {
  const { data, check } = phrase.encode(kind, created, secret);
  return [...data, ...check].map((word) => wordlist[word]).join(" ");
};
const chain = Array.from({ length: 64 }, (_, index) => {
  const digest = sha256(utf8(`dassie identity vector ${index}`));
  const secret = digest.subarray(0, 16);
  const kind = 1 + (index % 2);
  const realm = phrase.KIND_REALMS[kind];
  const created = new DataView(digest.buffer, digest.byteOffset + 16, 4).getUint32(0) &
    phrase.MOST_CREATED;
  const masterSeed = phrase.masterSeed(secret, realm);
  const nodeKey = fold(masterSeed, ["node"]);
  const publicKey = ed25519.getPublicKey(nodeKey);
  const entry = {
    index,
    kind,
    realm,
    created,
    secret: hex(secret),
    phrase: words(kind, created, secret),
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
write("chain.json", { chain });

// The phrase format on its own (spec/decisions/phrase-format.md). Places
// count from 0, the first word; a word's value is its index in the list.

const zeros = (length) => new Array(length).fill(0);
const SECRETS = [
  ["zero", new Uint8Array(16)],
  ["all ones", new Uint8Array(16).fill(0xff)],
  ["the top bit", Uint8Array.from([0x80, ...zeros(15)])],
  ["the bottom bit", Uint8Array.from([...zeros(15), 1])],
  ["alternating", new Uint8Array(16).fill(0xa5)],
  ...[0, 1, 2].map((index) => [
    `counter ${index}`,
    sha256(utf8(`dassie phrase vector ${index}`)).subarray(0, 16),
  ]),
];
const secretOf = (name) => SECRETS.find(([label]) => label === name)[1];
const NOW = phrase.hourOf(Date.UTC(2026, 9, 27, 13, 45));
const CREATED = [0, 1, NOW, 0x55555, 0xaaaaa, phrase.MOST_CREATED];

const made = (kind, created, secret) => {
  const { data, check } = phrase.encode(kind, created, secret);
  const indices = [...data, ...check];
  if (phrase.syndromes(indices).some((value) => value !== 0)) throw new Error("not a codeword");
  return indices;
};
const spell = (indices) => indices.map((word) => wordlist[word]);

// Phrases made from a kind, a creation hour and a secret: every secret in
// both realms, and one secret at every edge of the creation hour.
const madePhrases = [];
SECRETS.forEach(([name, secret], index) => {
  const hours = name === "counter 0" ? CREATED : [CREATED[index % CREATED.length]];
  for (const created of hours) {
    for (const kind of [1, 2]) {
      const indices = made(kind, created, secret);
      const realm = phrase.KIND_REALMS[kind];
      madePhrases.push({
        secretName: name,
        kind,
        realm,
        created,
        createdAt: phrase.hourStart(created),
        date: phrase.dateOf(created),
        secret: hex(secret),
        dataWords: indices.slice(0, phrase.DATA_WORDS),
        checkWords: indices.slice(phrase.DATA_WORDS),
        words: spell(indices),
        phrase: spell(indices).join(" "),
        masterSeed: hex(phrase.masterSeed(secret, realm)),
      });
    }
  }
});

// The hour of a clock reading, and the readings outside the field, at
// which a generator makes no phrase (`hour` null).
const clocks = [
  Date.UTC(2025, 11, 31, 23, 59, 59, 999),
  Date.UTC(2026, 0, 1),
  Date.UTC(2026, 0, 1, 0, 59, 59, 999),
  Date.UTC(2026, 0, 1, 1),
  Date.UTC(2026, 9, 27, 13, 45),
  Date.UTC(2145, 7, 15, 15, 59, 59, 999),
  Date.UTC(2145, 7, 15, 16),
].map((ms) => ({
  time: new Date(ms).toISOString(),
  unixMillis: ms,
  hour: phrase.hourOf(ms) ?? null,
}));

// Reading. A wrong word is another word of the list, picked from a hash of
// the case's name; an unreadable word is `null` (marked by the owner) or a
// word not in the list.
const pick = (name, place, avoid) => {
  for (let round = 0;; round += 1) {
    const digest = sha256(utf8(`dassie phrase wrong word ${name} ${place} ${round}`));
    const word = ((digest[0] << 8) | digest[1]) & 0x7ff;
    if (word !== avoid) return word;
  }
};
const listed = phrase.wordIndex(wordlist);
const UNKNOWN = ["dassie", "xxxx", "qwerty", "zzz", "kopje", "hyrax"];
for (const word of UNKNOWN) {
  if (listed.has(word.toLowerCase().slice(0, 4))) throw new Error(`${word} is in the list`);
}

const A = made(1, NOW, secretOf("counter 0"));
const B = made(2, NOW, secretOf("counter 1"));
// A reader's clock, in milliseconds since the Unix epoch: by default a day
// and a half after A and B were made.
const clockAt = (hour, minutes = 0) => phrase.EPOCH_MS + hour * phrase.HOUR_MS + minutes * 60_000;
const LATER = clockAt(NOW + 36, 30);

// `indices` spelled out and damaged at places: `wrong` takes another word,
// `unknown` a word not in the list, `marked` null.
const damage = (name, indices, { wrong = [], unknown = [], marked = [] }) => {
  const input = spell(indices);
  for (const place of wrong) input[place] = wordlist[pick(name, place, indices[place])];
  unknown.forEach((place, at) => (input[place] = UNKNOWN[at % UNKNOWN.length]));
  for (const place of marked) input[place] = null;
  return input;
};

// Each case gives the reader's realm and clock (milliseconds since the Unix
// epoch), the words read, and what the reader makes of them. `expect` is
// what the case was built to give, checked here apart from the decoder:
// "ok" (the source back, unchanged), "corrected" (the source back,
// changed), or a refusal.
const reading = [];
const readCase = (name, input, { realm = "test", clock = LATER, source, expect }) => {
  const outcome = phrase.read(wordlist, input, { realm, clock });
  if (expect === "ok" || expect === "corrected") {
    if (outcome.refused !== undefined) throw new Error(`${name}: refused ${outcome.refused}`);
    if (outcome.phrase !== spell(source).join(" ")) throw new Error(`${name}: another phrase`);
    if (outcome.corrected !== (expect === "corrected")) throw new Error(`${name}: corrected?`);
  } else if (outcome.refused !== expect) {
    throw new Error(`${name}: ${outcome.refused ?? "read"}, not ${expect}`);
  }
  const result = outcome.refused === undefined
    ? {
      phrase: outcome.phrase,
      corrected: outcome.corrected,
      changed: outcome.changed,
      kind: outcome.kind,
      realm: outcome.realm,
      created: outcome.created,
      date: outcome.date,
      secret: hex(outcome.secret),
      masterSeed: hex(outcome.masterSeed),
    }
    : outcome.refused === "tooDamaged"
    ? { refused: "tooDamaged" }
    : outcome;
  reading.push({ name, realm, clockUnixMs: clock, input, result });
};

// A refusal as too damaged that no decoder within the bound can avoid: no
// codeword lies within the errors it may correct of what was read.
const tooDamaged = (name, source, damaged, options = {}) => {
  const erasures = (damaged.unknown?.length ?? 0) + (damaged.marked?.length ?? 0);
  const wrong = damaged.wrong?.length ?? 0;
  const budget = phrase.errorBudget(erasures);
  if (budget >= 0 && !(wrong > budget && wrong + budget <= 6 - erasures)) {
    throw new Error(`${name}: not provably too damaged`);
  }
  readCase(name, damage(name, source, damaged), { ...options, source, expect: "tooDamaged" });
};
const corrected = (name, source, damaged, options = {}) =>
  readCase(name, damage(name, source, damaged), { ...options, source, expect: "corrected" });

readCase("clean, test realm", spell(A), { source: A, expect: "ok" });
readCase("clean, live realm", spell(B), { realm: "live", source: B, expect: "ok" });
readCase("upper case", spell(A).map((word) => word.toUpperCase()), { source: A, expect: "ok" });
readCase("first four letters", spell(A).map((word) => word.slice(0, 4)), {
  source: A,
  expect: "ok",
});
readCase(
  "mixed case, first four letters",
  spell(B).map((word, place) =>
    place % 2 === 0 ? word.slice(0, 4).toUpperCase() : word[0].toUpperCase() + word.slice(1)
  ),
  { realm: "live", source: B, expect: "ok" },
);
readCase(
  "letters after the fourth are not read",
  spell(A).map((word) => (word.length >= 4 ? `${word.slice(0, 4)}xyz` : word)),
  { source: A, expect: "ok" },
);
// Only ASCII letters change case: the Kelvin sign, whose Unicode lower case
// is `k`, makes a word that is not in the list, so it reads as unreadable.
const kelvinSecret = [...Array(64).keys()]
  .map((index) => sha256(utf8(`dassie phrase kelvin ${index}`)).subarray(0, 16))
  .find((secret) => spell(made(1, NOW, secret)).some((word) => word.startsWith("k")));
const K = made(1, NOW, kelvinSecret);
const kelvinAt = spell(K).findIndex((word) => word.startsWith("k"));
readCase(
  "a Kelvin sign is no k",
  spell(K).map((word, place) => (place === kelvinAt ? `K${word.slice(1)}` : word)),
  { source: K, expect: "corrected" },
);

for (const place of [0, 7, 13, 14, 19]) {
  corrected(`one wrong word at ${place}`, A, { wrong: [place] });
}
corrected("two wrong words, first and last", A, { wrong: [0, 19] });
corrected("two wrong words, side by side", A, { wrong: [3, 4] });
corrected("two wrong check words", B, { wrong: [14, 15] }, { realm: "live" });
corrected("two wrong words, data and check", B, { wrong: [12, 18] }, { realm: "live" });

corrected("one marked word", A, { marked: [0] });
corrected("one word not in the list", A, { unknown: [5] });
corrected("two unreadable words", A, { marked: [1], unknown: [17] });
corrected("three unreadable words", B, { marked: [2, 9], unknown: [13] }, { realm: "live" });
corrected("four unreadable words", A, { marked: [0, 6, 14], unknown: [19] });
corrected("five unreadable words, the first five", A, { marked: [0, 1, 2, 3, 4] });
corrected("five unreadable words, check words", B, { unknown: [14, 15, 16, 17, 18] }, {
  realm: "live",
});
corrected("five unreadable words, spread", A, { marked: [0, 5, 10], unknown: [15, 19] });
const plain = made(1, 0, secretOf("zero"));
if (plain[5] !== 0) throw new Error("the zero secret's sixth word is the list's first");
corrected("an unreadable word that is the list's first", plain, { marked: [5] });

corrected("one wrong word and one unreadable", A, { wrong: [8], marked: [2] });
corrected("one wrong word and two unreadable", B, { wrong: [19], marked: [0], unknown: [10] }, {
  realm: "live",
});
corrected("one wrong word and three unreadable", A, {
  wrong: [11],
  marked: [1, 4],
  unknown: [16],
});
corrected("two wrong words and one unreadable", A, { wrong: [6, 15], marked: [9] });

tooDamaged("six unreadable words", A, { marked: [0, 3, 6, 9, 12, 15] });
tooDamaged("six unreadable check words", A, { unknown: [14, 15, 16, 17, 18, 19] });
tooDamaged("one wrong word and four unreadable", A, { wrong: [7], marked: [0, 1, 2, 3] });
tooDamaged("two wrong words and two unreadable", B, { wrong: [5, 6], marked: [10, 11] }, {
  realm: "live",
});
tooDamaged("three wrong words", A, { wrong: [0, 9, 19] });
tooDamaged("three wrong data words", B, { wrong: [1, 2, 3] }, { realm: "live" });
tooDamaged("three wrong check words", A, { wrong: [14, 16, 18] });
tooDamaged("three wrong words and one unreadable", A, { wrong: [2, 8, 17], marked: [5] });
tooDamaged("four wrong words", A, { wrong: [0, 5, 10, 15] });
tooDamaged("no word in the list", A, {
  unknown: Array.from({ length: 20 }, (_, place) => place),
});
tooDamaged("every word marked", A, {
  marked: Array.from({ length: 20 }, (_, place) => place),
});

for (const bytes of [16, 20, 24, 28, 32]) {
  const entropy = sha256(utf8(`dassie phrase bip39 ${bytes}`)).subarray(0, bytes);
  const input = entropyToMnemonic(entropy, wordlist).split(" ");
  readCase(`a BIP-39 phrase of ${input.length} words`, input, { expect: "bip39" });
}
readCase("a phrase and one word more", [...spell(A), wordlist[0]], { expect: "bip39" });
for (const length of [0, 1, 11, 13, 19, 22, 25]) {
  const input = [...spell(A), ...spell(B)].slice(0, length);
  readCase(`${length} words`, input, { expect: "length" });
}

const kindZero = made(0, NOW, secretOf("counter 2"));
readCase("kind 0", spell(kindZero), { expect: "kindZero" });
readCase("kind 0, with a wrong word", damage("kind 0", kindZero, { wrong: [10] }), {
  expect: "kindZero",
});
for (const kind of [3, 42, 63]) {
  readCase(`kind ${kind}`, spell(made(kind, NOW, secretOf("counter 2"))), {
    expect: "unassignedKind",
  });
}
readCase(
  "kind 63, with two unreadable words",
  damage("kind 63", made(63, NOW, secretOf("counter 2")), { marked: [0, 1] }),
  { expect: "unassignedKind" },
);
readCase("a live phrase where a test phrase is expected", spell(B), { expect: "otherRealm" });
readCase("a test phrase where a live phrase is expected", spell(A), {
  realm: "live",
  expect: "otherRealm",
});
readCase(
  "a test phrase with a wrong word where a live phrase is expected",
  damage("other realm", A, { wrong: [3] }),
  { realm: "live", expect: "otherRealm" },
);

readCase("made in the reader's hour", spell(A), { clock: clockAt(NOW, 59), source: A, expect: "ok" });
readCase("made an hour after the reader's clock", spell(A), {
  clock: clockAt(NOW - 1, 30),
  source: A,
  expect: "ok",
});
readCase("made exactly a day after the reader's clock", spell(A), {
  clock: clockAt(NOW - 24),
  source: A,
  expect: "ok",
});
readCase("made a day and a millisecond after the reader's clock", spell(A), {
  clock: clockAt(NOW - 24) - 1,
  expect: "createdInFuture",
});
readCase("made a day and a half after the reader's clock", spell(B), {
  realm: "live",
  clock: clockAt(NOW - 36, 30),
  expect: "createdInFuture",
});
const last = made(1, phrase.MOST_CREATED, secretOf("counter 2"));
readCase("made in the field's last hour, read at its first", spell(last), {
  clock: clockAt(0),
  expect: "createdInFuture",
});
readCase("made in the field's last hour, read in it", spell(last), {
  clock: clockAt(phrase.MOST_CREATED, 30),
  source: last,
  expect: "ok",
});
const first = made(2, 0, secretOf("counter 2"));
readCase("made in the field's first hour, read at its last", spell(first), {
  realm: "live",
  clock: clockAt(phrase.MOST_CREATED, 59),
  source: first,
  expect: "ok",
});
readCase("made in the field's first hour, read by a clock at 1970", spell(first), {
  realm: "live",
  clock: 0,
  expect: "createdInFuture",
});
readCase(
  "made after the reader's clock, with a wrong word",
  damage("future", A, { wrong: [16] }),
  { clock: clockAt(0), expect: "createdInFuture" },
);
readCase(
  "a test phrase made after the reader's clock where a live phrase is expected",
  spell(A),
  { realm: "live", clock: clockAt(0), expect: "otherRealm" },
);

// Every phrase made reads back as itself in its own realm.
for (const entry of madePhrases) {
  const outcome = phrase.read(wordlist, entry.words, {
    realm: entry.realm,
    clock: clockAt(phrase.MOST_CREATED),
  });
  if (
    outcome.phrase !== entry.phrase || outcome.corrected ||
    hex(outcome.masterSeed) !== entry.masterSeed
  ) {
    throw new Error(`${entry.phrase} does not read back`);
  }
}

write("phrase.json", { made: madePhrases, clocks, reading });

const message = utf8("dassie test message");
const keys = [fill(1), fill(3)].map((seed) => {
  const publicKey = ed25519.getPublicKey(seed);
  return {
    seed: hex(seed),
    publicKey: hex(publicKey),
    nodeId: nodeId(publicKey),
    message: hex(message),
    signature: hex(ed25519.sign(message, seed)),
  };
});
write("keys.json", {
  keys,
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
