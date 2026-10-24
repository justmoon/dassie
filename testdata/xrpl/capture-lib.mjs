// What the capture scripts of testdata/xrpl share
// (spec/designs/xrpl-validation.md#13-what-pins-it): a WebSocket client
// for rippled's API, the ledger's hashes and binary format as far as the
// captures read them, and node public keys in base58. Node alone, with no
// packages: `nix develop -c node testdata/xrpl/capture-<fixture>.mjs`.
//
// The WebSocket client is its own, on a TLS socket: Node's built-in one
// writes `Connection: upgrade` in lower case, which the proof servers'
// front (resources/proof-server/Caddyfile) matches case-sensitively, and
// so does not take for an upgrade.

import { createHash, randomBytes } from "node:crypto";
import { connect as tlsConnect } from "node:tls";

export const MAINNET = process.env.XRPL_MAINNET ?? "wss://s2.ripple.com";
export const TESTNET = process.env.XRPL_TESTNET ?? "wss://s.altnet.rippletest.net:51233";

export function assert(condition, message) {
  if (!condition) throw new Error(`assertion failed: ${message}`);
}

// The command line's `--name value` pairs, each name one of `names`.
export function options(names) {
  const args = process.argv.slice(2);
  assert(args.length % 2 === 0, `usage: --name value ... (${names.join(", ")})`);
  const result = {};
  for (let at = 0; at < args.length; at += 2) {
    const name = args[at].replace(/^--/, "");
    assert(names.includes(name), `unknown option ${args[at]} (${names.join(", ")})`);
    result[name] = args[at + 1];
  }
  return result;
}

// A ledger index from the command line, or "validated".
export function ledgerOption(value) {
  if (value === undefined) return "validated";
  assert(/^[0-9]{1,10}$/.test(value), `a ledger index: ${value}`);
  return Number(value);
}

export function print(value) {
  process.stdout.write(`${JSON.stringify(value, null, 2)}\n`);
}

export const hex = (bytes) => Buffer.from(bytes).toString("hex").toUpperCase();
export const unhex = (text) => {
  assert(/^([0-9A-Fa-f]{2})*$/.test(text), "hex");
  return Buffer.from(text, "hex");
};

export function sha512Half(...parts) {
  const hash = createHash("sha512");
  for (const part of parts) hash.update(part);
  return hash.digest().subarray(0, 32);
}

const sha256 = (bytes) => createHash("sha256").update(bytes).digest();

// ---------------------------------------------------------------------------
// WebSocket

const MAX_MESSAGE = 64 * 1024 * 1024;
const TIMEOUT_MS = 60_000;

// A connection to one server: `request` sends a command and resolves to
// the whole reply, `onStream` takes every message that answers no request.
export async function open(url) {
  const { hostname, port, pathname } = new URL(url);
  assert(url.startsWith("wss://"), `a wss:// URL: ${url}`);
  const socket = tlsConnect({ host: hostname, port: Number(port || 443), servername: hostname, ALPNProtocols: ["http/1.1"] });
  const key = randomBytes(16).toString("base64");
  await new Promise((resolve, reject) => {
    socket.once("secureConnect", resolve);
    socket.once("error", reject);
  });
  socket.write(
    `GET ${pathname || "/"} HTTP/1.1\r\nHost: ${hostname}\r\nUpgrade: websocket\r\n` +
      `Connection: Upgrade\r\nSec-WebSocket-Key: ${key}\r\nSec-WebSocket-Version: 13\r\n\r\n`,
  );

  let buffer = Buffer.alloc(0);
  let upgraded = false;
  let fragments = [];
  let nextId = 1;
  const pending = new Map();
  const connection = { onStream: () => {}, closed: false };
  let failure = null;

  const fail = (error) => {
    failure ??= error;
    connection.closed = true;
    for (const { reject } of pending.values()) reject(failure);
    pending.clear();
    socket.destroy();
  };

  const send = (opcode, payload) => {
    assert(payload.length < 65536, "a short frame");
    const mask = randomBytes(4);
    const header = payload.length < 126
      ? Buffer.from([0x80 | opcode, 0x80 | payload.length])
      : Buffer.from([0x80 | opcode, 0x80 | 126, payload.length >> 8, payload.length & 0xff]);
    const masked = Buffer.from(payload);
    for (let at = 0; at < masked.length; at++) masked[at] ^= mask[at % 4];
    socket.write(Buffer.concat([header, mask, masked]));
  };

  const deliver = (text) => {
    const message = JSON.parse(text);
    const waiting = message.type === "response" ? pending.get(message.id) : undefined;
    if (waiting === undefined) {
      connection.onStream(message);
      return;
    }
    pending.delete(message.id);
    clearTimeout(waiting.timer);
    waiting.resolve(message);
  };

  // Each whole frame in the buffer, or nothing until more arrives.
  const frames = () => {
    for (;;) {
      if (buffer.length < 2) return;
      const fin = (buffer[0] & 0x80) !== 0;
      const opcode = buffer[0] & 0x0f;
      assert((buffer[1] & 0x80) === 0, "the server does not mask");
      let length = buffer[1] & 0x7f;
      let at = 2;
      if (length === 126) {
        if (buffer.length < 4) return;
        length = buffer.readUInt16BE(2);
        at = 4;
      } else if (length === 127) {
        if (buffer.length < 10) return;
        const long = buffer.readBigUInt64BE(2);
        assert(long <= BigInt(MAX_MESSAGE), "a frame within bounds");
        length = Number(long);
        at = 10;
      }
      if (buffer.length < at + length) return;
      const payload = buffer.subarray(at, at + length);
      buffer = buffer.subarray(at + length);
      if (opcode === 0x8) return fail(new Error(`${url} closed the connection`));
      if (opcode === 0x9) {
        send(0xa, payload);
        continue;
      }
      if (opcode === 0xa) continue;
      assert(opcode === 0x0 || opcode === 0x1, `a text frame, not opcode ${opcode}`);
      fragments.push(Buffer.from(payload));
      assert(fragments.reduce((sum, part) => sum + part.length, 0) <= MAX_MESSAGE, "a message within bounds");
      if (fin) {
        const text = Buffer.concat(fragments).toString("utf8");
        fragments = [];
        deliver(text);
      }
    }
  };

  await new Promise((resolve, reject) => {
    socket.on("data", (chunk) => {
      try {
        buffer = Buffer.concat([buffer, chunk]);
        if (!upgraded) {
          const end = buffer.indexOf("\r\n\r\n");
          if (end < 0) return;
          const head = buffer.subarray(0, end).toString("latin1");
          buffer = buffer.subarray(end + 4);
          const accept = createHash("sha1").update(`${key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`).digest("base64");
          if (!/^HTTP\/1\.1 101 /.test(head) || !head.toLowerCase().includes(`sec-websocket-accept: ${accept.toLowerCase()}`)) {
            throw new Error(`${url} did not upgrade: ${head.split("\r\n")[0]}`);
          }
          upgraded = true;
          resolve();
        }
        frames();
      } catch (error) {
        reject(error);
        fail(error);
      }
    });
    socket.on("error", (error) => {
      reject(error);
      fail(error);
    });
    socket.on("close", () => fail(new Error(`${url} closed the connection`)));
  });

  connection.request = (command) => {
    if (connection.closed) return Promise.reject(failure);
    const id = nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(new Error(`${url}: no answer to ${command.command} in ${TIMEOUT_MS} ms`));
      }, TIMEOUT_MS);
      pending.set(id, { resolve, reject, timer });
      send(0x1, Buffer.from(JSON.stringify({ id, ...command }), "utf8"));
    });
  };
  // The result of a command at API version 1, which must succeed.
  connection.call = async (command, params = {}) => {
    const reply = await connection.request({ command, api_version: 1, ...params });
    if (reply.status !== "success") {
      throw new Error(`${url} ${command}: ${JSON.stringify(reply).slice(0, 500)}`);
    }
    return reply.result;
  };
  connection.close = () => {
    connection.closed = true;
    socket.destroy();
  };
  return connection;
}

// ---------------------------------------------------------------------------
// Ledgers

// The header of a ledger, as JSON and in binary.
export async function ledgerHeader(connection, ledgerIndex) {
  const json = await connection.call("ledger", { ledger_index: ledgerIndex });
  assert(json.validated === true, "a validated ledger");
  const index = Number(json.ledger.ledger_index);
  const binary = await connection.call("ledger", { ledger_index: index, binary: true });
  assert(binary.ledger_hash === json.ledger_hash, "one ledger");
  assert(binary.ledger.ledger_data.length === 2 * 118, "a header of 118 bytes");
  return { index, hash: json.ledger_hash, json: json.ledger, data: binary.ledger.ledger_data };
}

// The header's fields as the `ledger` method gives them.
export function headerFields(json) {
  return {
    account_hash: json.account_hash,
    close_flags: json.close_flags,
    close_time: json.close_time,
    close_time_resolution: json.close_time_resolution,
    parent_close_time: json.parent_close_time,
    parent_hash: json.parent_hash,
    total_coins: json.total_coins,
    transaction_hash: json.transaction_hash,
  };
}

// A ledger's transactions, each with its metadata, in the order the
// `ledger` method lists them.
export async function transactions(connection, ledgerIndex) {
  const result = await connection.call("ledger", { ledger_index: ledgerIndex, transactions: true, expand: true, binary: true });
  assert(result.validated === true, "a validated ledger");
  return result.ledger.transactions.map(({ tx_blob, meta }) => ({ tx_blob, meta }));
}

const PREFIX_TRANSACTION_ID = Buffer.from("TXN\0", "latin1");
const PREFIX_TX_NODE = Buffer.from("SND\0", "latin1");
const PREFIX_INNER_NODE = Buffer.from("MIN\0", "latin1");
const ZERO_HASH = Buffer.alloc(32);
const MAX_DEPTH = 64;

export const transactionId = (blob) => sha512Half(PREFIX_TRANSACTION_ID, unhex(blob));

// The length prefix of a variable-length field.
function lengthPrefix(length) {
  if (length <= 192) return Buffer.from([length]);
  if (length <= 12480) {
    const rest = length - 193;
    return Buffer.from([193 + (rest >> 8), rest & 0xff]);
  }
  assert(length <= 918744, "a length the format can hold");
  const rest = length - 12481;
  return Buffer.from([241 + (rest >> 16), (rest >> 8) & 0xff, rest & 0xff]);
}

const nibble = (key, depth) => (depth % 2 === 0 ? key[depth >> 1] >> 4 : key[depth >> 1] & 0x0f);

// A ledger's transaction tree, built as rippled builds it: each leaf hangs
// at the first depth no other key shares.
export function transactionTree(entries) {
  const root = { children: new Array(16).fill(null) };
  for (const { tx_blob, meta } of entries) {
    const key = transactionId(tx_blob);
    const blob = unhex(tx_blob);
    const metadata = unhex(meta);
    const hash = sha512Half(PREFIX_TX_NODE, lengthPrefix(blob.length), blob, lengthPrefix(metadata.length), metadata, key);
    let node = root;
    let leaf = { key, hash };
    for (let depth = 0;; depth++) {
      assert(depth < MAX_DEPTH, "a depth within the key");
      const slot = nibble(key, depth);
      const child = node.children[slot];
      if (child === null) {
        node.children[slot] = leaf;
        break;
      }
      if (child.children) {
        node = child;
        continue;
      }
      assert(!child.key.equals(key), "one leaf per key");
      const inner = { children: new Array(16).fill(null) };
      inner.children[nibble(child.key, depth + 1)] = child;
      node.children[slot] = inner;
      node = inner;
    }
  }
  const hashOf = (node) => {
    if (node === null) return ZERO_HASH;
    if (!node.children) return node.hash;
    node.hash = sha512Half(PREFIX_INNER_NODE, ...node.children.map(hashOf));
    return node.hash;
  };
  hashOf(root);
  return root;
}

// The inner nodes on `key`'s path, root first, each as its sixteen
// children's hashes.
export function branchLevels(root, key) {
  const levels = [];
  let node = root;
  for (let depth = 0; node.children; depth++) {
    assert(depth < MAX_DEPTH, "a depth within the key");
    levels.push(node.children.map((child) => hex(child === null ? ZERO_HASH : child.hash)));
    node = node.children[nibble(key, depth)];
    assert(node !== null, "the key is in the tree");
  }
  assert(node.key.equals(key), "the key's own leaf");
  return { levels, leafHash: hex(node.hash) };
}

// ---------------------------------------------------------------------------
// The binary format, as far as the captures read it

const FIXED_WIDTH = {
  1: 2, // UInt16
  2: 4, // UInt32
  3: 8, // UInt64
  4: 16, // Hash128
  5: 32, // Hash256
  9: 12, // Number
  10: 4, // Int32
  11: 8, // Int64
  16: 1, // UInt8
  17: 20, // Hash160
  20: 12, // UInt96
  21: 24, // Hash192
  22: 48, // UInt384
  23: 64, // UInt512
  26: 20, // Currency
};
const VARIABLE = new Set([7, 8, 19]); // Blob, AccountID, Vector256
const OBJECT = 14;
const ARRAY = 15;
const PATH_SET = 18;
const ISSUE = 24;
const BRIDGE = 25;
const AMOUNT = 6;
const NO_ACCOUNT = Buffer.concat([Buffer.alloc(19), Buffer.from([1])]);

function fieldHeader(bytes, at) {
  assert(at < bytes.length, "a field header");
  let type = bytes[at] >> 4;
  let field = bytes[at] & 0x0f;
  let next = at + 1;
  if (type === 0) type = bytes[next++];
  if (field === 0) field = bytes[next++];
  assert(next <= bytes.length, "a whole field header");
  return { type, field, next };
}

function lengthAt(bytes, at) {
  const first = bytes[at];
  if (first <= 192) return { length: first, next: at + 1 };
  if (first <= 240) return { length: 193 + (first - 193) * 256 + bytes[at + 1], next: at + 2 };
  assert(first <= 254, "a length prefix");
  return { length: 12481 + (first - 241) * 65536 + bytes[at + 1] * 256 + bytes[at + 2], next: at + 3 };
}

function issueEnd(bytes, at) {
  const currency = bytes.subarray(at, at + 20);
  if (currency.equals(Buffer.alloc(20))) return at + 20;
  // An MPT's issue: its issuer, the account no one holds, its sequence.
  if (bytes.subarray(at + 20, at + 40).equals(NO_ACCOUNT)) return at + 44;
  return at + 40;
}

// Where the value of a field of `type` at `at` ends; `types` gathers the
// types it holds, nested ones too.
function valueEnd(bytes, type, at, types) {
  types.add(type);
  if (FIXED_WIDTH[type] !== undefined) return at + FIXED_WIDTH[type];
  if (VARIABLE.has(type)) {
    const { length, next } = lengthAt(bytes, at);
    return next + length;
  }
  if (type === AMOUNT) {
    if (bytes[at] & 0x80) return at + 48;
    if (bytes[at] & 0x20) return at + 33;
    return at + 8;
  }
  if (type === ISSUE) return issueEnd(bytes, at);
  if (type === BRIDGE) {
    let next = at;
    for (let side = 0; side < 2; side++) {
      const { length, next: start } = lengthAt(bytes, next);
      next = issueEnd(bytes, start + length);
    }
    return next;
  }
  if (type === PATH_SET) {
    let next = at;
    for (let steps = 0; steps < 10_000; steps++) {
      const flags = bytes[next++];
      if (flags === 0x00) return next;
      if (flags === 0xff) continue;
      next += 20 * ((flags & 0x01 ? 1 : 0) + (flags & 0x10 ? 1 : 0) + (flags & 0x20 ? 1 : 0));
    }
    throw new Error("a path set within bounds");
  }
  if (type === OBJECT || type === ARRAY) {
    const end = type === OBJECT ? 0xe1 : 0xf1;
    let next = at;
    for (let count = 0; count < 100_000; count++) {
      assert(next < bytes.length, "an end marker");
      if (bytes[next] === end) return next + 1;
      const header = fieldHeader(bytes, next);
      next = valueEnd(bytes, header.type, header.next, types);
    }
    throw new Error("an object within bounds");
  }
  throw new Error(`a type the captures know: ${type}`);
}

// The fields of a serialized object, each with its type and code and its
// bytes, and every type the object holds.
export function fields(bytes) {
  const list = [];
  const types = new Set();
  let at = 0;
  while (at < bytes.length) {
    const start = at;
    const { type, field, next } = fieldHeader(bytes, at);
    at = valueEnd(bytes, type, next, types);
    assert(at <= bytes.length, "a whole field");
    list.push({ type, field, start, valueStart: next, end: at });
  }
  return { list, types };
}

// ---------------------------------------------------------------------------
// Keys and manifests

const ALPHABET = "rpshnaf39wBUDNEGHJKLM4PQRST7VWXYZ2bcdeCg65jkm8oFqi1tuvAxyz";
const NODE_PUBLIC = 0x1c;

function base58Check(payload) {
  const bytes = Buffer.concat([payload, sha256(sha256(payload)).subarray(0, 4)]);
  let number = BigInt(`0x${bytes.toString("hex")}`);
  let text = "";
  while (number > 0n) {
    text = ALPHABET[Number(number % 58n)] + text;
    number /= 58n;
  }
  for (const byte of bytes) {
    if (byte !== 0) break;
    text = ALPHABET[0] + text;
  }
  return text;
}

export function nodePublic(key) {
  const bytes = typeof key === "string" ? unhex(key) : key;
  assert(bytes.length === 33, "a public key of 33 bytes");
  return base58Check(Buffer.concat([Buffer.from([NODE_PUBLIC]), bytes]));
}

const MANIFEST_PREFIX = Buffer.from("MAN\0", "latin1");
const SEQUENCE = "2:4";
const PUBLIC_KEY = "7:1";
const SIGNING_PUB_KEY = "7:3";
const SIGNATURE = "7:6";
const DOMAIN = "7:7";
const MASTER_SIGNATURE = "7:18";

// A manifest's keys, sequence and domain, and the bytes its keys sign:
// the prefix and its fields without the two signatures.
export function manifest(bytes) {
  const { list } = fields(bytes);
  const value = (code) => {
    const found = list.find(({ type, field }) => `${type}:${field}` === code);
    if (found === undefined) return undefined;
    if (found.type === 7) {
      const { length, next } = lengthAt(bytes, found.valueStart);
      return bytes.subarray(next, next + length);
    }
    return bytes.subarray(found.valueStart, found.end);
  };
  const master = value(PUBLIC_KEY);
  const signing = value(SIGNING_PUB_KEY);
  assert(master !== undefined && signing !== undefined, "a manifest with both keys");
  const domain = value(DOMAIN);
  const signed = list
    .filter(({ type, field }) => ![SIGNATURE, MASTER_SIGNATURE].includes(`${type}:${field}`))
    .map(({ start, end }) => bytes.subarray(start, end));
  return {
    master: hex(master),
    master_b58: nodePublic(master),
    signing: hex(signing),
    signing_b58: nodePublic(signing),
    sequence: value(SEQUENCE).readUInt32BE(0),
    domain: domain === undefined ? null : domain.toString("latin1"),
    signed_bytes: hex(Buffer.concat([MANIFEST_PREFIX, ...signed])),
  };
}

// ---------------------------------------------------------------------------
// Publishers and pools

// Each network's publishers' sites, in the order its config names them.
export const PUBLISHERS = {
  mainnet: ["https://vl.ripple.com", "https://unl.xrplf.org"],
  testnet: ["https://vl.altnet.rippletest.net"],
};

// A signed validator list, as its site serves it.
export async function fetchList(site) {
  const response = await fetch(site, { signal: AbortSignal.timeout(TIMEOUT_MS) });
  assert(response.ok, `${site} answered ${response.status}`);
  return response.text();
}

// The validators a list body names, with its publisher's manifest.
export function listValidators(body) {
  const list = JSON.parse(body);
  const blob = list.blob ?? list.blobs_v2?.[0]?.blob;
  assert(typeof blob === "string", "a list with a blob");
  const inner = JSON.parse(Buffer.from(blob, "base64").toString("utf8"));
  return { manifest: list.manifest, validators: inner.validators.map((v) => v.validation_public_key) };
}

// A validator's live manifest, from the `manifest` method.
export async function liveManifest(connection, master) {
  const result = await connection.call("manifest", { public_key: nodePublic(master) });
  assert(typeof result.manifest === "string", `a manifest of ${master}`);
  return Buffer.from(result.manifest, "base64");
}

export const RLUSD = { currency: "524C555344000000000000000000000000000000", issuer: "rMxCKbEDwqr76QuheSUMdEGf4B9xJ8m5De" };
export const XRP = { currency: "XRP" };

// The XRP/RLUSD pool's ledger objects at `ledgerIndex`, from `ledger_entry`
// with binary: its account's root, its RLUSD line and its AMM object, and
// RLUSD's issuer's root, each answer whole.
export async function poolObjects(connection, ledgerIndex) {
  const at = { ledger_index: ledgerIndex };
  const info = await connection.call("amm_info", { asset: XRP, asset2: RLUSD, ...at });
  const account = info.amm.account;
  const entry = async (params) => {
    const result = await connection.call("ledger_entry", { ...params, binary: true, ...at });
    assert(typeof result.node_binary === "string", "an object in binary");
    return result;
  };
  return {
    info,
    root: await entry({ account_root: account }),
    line: await entry({ ripple_state: { accounts: [account, RLUSD.issuer], currency: RLUSD.currency } }),
    amm: await entry({ amm: { asset: XRP, asset2: RLUSD } }),
    issuer: await entry({ account_root: RLUSD.issuer }),
  };
}

export const POOL_LABELS = {
  root: "AccountRoot (RLUSD/XRP AMM account)",
  line: "RippleState (AMM account / RLUSD issuer)",
  amm: "AMM object (XRP/RLUSD)",
  issuer: "AccountRoot (RLUSD issuer)",
};
