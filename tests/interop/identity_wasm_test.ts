// The golden vectors through the WASM builds: the bytes must be the ones
// the native tests check (crates/*/tests/data), so the wallet derives
// exactly what the node derives.

import { test } from "node:test";
import { readFile } from "node:fs/promises";

import { assert, assertEquals, assertFalse } from "@std/assert";

import {
  decodeIlpPacket,
  deriveNodeKey,
  encodeFulfill,
  encodePrepare,
  encodeReject,
  loginSecret,
  nodeId,
  normalizeMnemonic,
  publicKey,
  sign,
  validateMnemonic,
} from "@dassie/wasm";

const crates = new URL("../../crates/", import.meta.url);

// eslint-disable-next-line typescript/no-explicit-any
async function read(path: string): Promise<any> {
  return JSON.parse(await readFile(new URL(path, crates), "utf8"));
}

// The identity vectors, from the generator of our own
// (testdata/identity/generate-vectors.mjs).
// eslint-disable-next-line typescript/no-explicit-any
async function vectors(path: string): Promise<any> {
  const document = await read(`dassie-identity/tests/data/${path}`);
  assertEquals(document.generator, "testdata/identity/generate-vectors.mjs");
  return document;
}

test("the passphrase chain matches the vectors", async () => {
  const { chain } = await vectors("chain.json");
  assert(chain.length > 60);
  for (const entry of chain) {
    assert(validateMnemonic(entry.mnemonic));
    const nodeKey = deriveNodeKey(entry.mnemonic);
    assertEquals(nodeKey.toHex(), entry.nodeKey);
    assertEquals(publicKey(nodeKey).toHex(), entry.publicKey);
    assertEquals(nodeId(publicKey(nodeKey)), entry.nodeId);
    assertEquals(loginSecret(nodeKey).toHex(), entry.loginSecret);
  }
});

test("phrases are validated and canonicalised", async () => {
  const { chain } = await vectors("chain.json");
  const phrase: string = chain[0].mnemonic;
  assertEquals(normalizeMnemonic(`  ${phrase.toUpperCase()}  `), phrase);
  // Only a valid BIP-39 phrase is a passphrase.
  const words = phrase.split(" ");
  assertFalse(validateMnemonic([...words.slice(0, 11), words[0]].join(" ")));
  assertFalse(validateMnemonic("not a passphrase"));
});

test("keys and node IDs match the vectors", async () => {
  const { keys } = await vectors("keys.json");
  for (const key of keys) {
    const seed = Uint8Array.fromHex(key.seed);
    assertEquals(publicKey(seed).toHex(), key.publicKey);
    assertEquals(nodeId(publicKey(seed)), key.nodeId);
    assertEquals(
      sign(seed, Uint8Array.fromHex(key.message)).toHex(),
      key.signature,
    );
  }
});

test("ILP packets match the original", async () => {
  const { packets, source } = await read("dassie-ilp/tests/data/vectors.json");
  assertEquals(source, "e962ab78");
  for (const vector of packets) {
    const bytes = Uint8Array.fromHex(vector.encoded);
    const packet = decodeIlpPacket(bytes);
    const data = vector.packet.data;
    let encoded: Uint8Array;
    if (packet.type === 12) {
      assertEquals(packet.address, data.destination);
      encoded = encodePrepare(
        BigInt(data.amount),
        packet.expiresAt,
        Uint8Array.fromHex(data.executionCondition),
        data.destination,
        Uint8Array.fromHex(data.data),
      );
    } else if (packet.type === 13) {
      encoded = encodeFulfill(
        Uint8Array.fromHex(data.fulfillment),
        Uint8Array.fromHex(data.data),
      );
    } else {
      assertEquals(packet.code, data.code);
      encoded = encodeReject(
        data.code,
        data.triggeredBy,
        data.message,
        Uint8Array.fromHex(data.data),
      );
    }
    assertEquals(encoded.toHex(), vector.encoded, vector.name);
  }
});
