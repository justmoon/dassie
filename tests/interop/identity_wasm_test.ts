// The golden vectors through the WASM builds: the bytes must be the ones
// the native tests check (crates/*/tests/data), so the wallet derives
// exactly what the node derives.

import { readFile } from "node:fs/promises";

import { assert, assertEquals, assertFalse } from "@std/assert";

import {
  decodeIlpPacket,
  deriveNodeKey,
  devSessionToken,
  encodeFulfill,
  encodePrepare,
  encodeReject,
  loginSecret,
  nodeId,
  normalizeMnemonic,
  publicKey,
  sign,
  validateMnemonic,
} from "@dassie/identity-wasm";

const crates = new URL("../../crates/", import.meta.url);

// deno-lint-ignore no-explicit-any
async function vectors(path: string): Promise<any> {
  const document = JSON.parse(await readFile(new URL(path, crates), "utf8"));
  assertEquals(document.source, "e962ab78");
  return document;
}

Deno.test("the passphrase chain matches the original", async () => {
  const { chain } = await vectors("dassie-identity/tests/data/chain.json");
  assert(chain.length > 60);
  for (const entry of chain) {
    assert(validateMnemonic(entry.mnemonic));
    const nodeKey = deriveNodeKey(entry.mnemonic);
    assertEquals(nodeKey.toHex(), entry.nodeKey);
    assertEquals(publicKey(nodeKey).toHex(), entry.publicKey);
    assertEquals(nodeId(publicKey(nodeKey)), entry.nodeId);
    assertEquals(loginSecret(nodeKey).toHex(), entry.loginSecret);
    assertEquals(
      devSessionToken(entry.mnemonic).toHex(),
      entry.devSessionToken,
    );
  }
});

Deno.test("phrases are validated and canonicalised", async () => {
  const { chain } = await vectors("dassie-identity/tests/data/chain.json");
  const phrase: string = chain[0].mnemonic;
  assertEquals(normalizeMnemonic(`  ${phrase.toUpperCase()}  `), phrase);
  // UI-5: the original accepted any text as a passphrase.
  const words = phrase.split(" ");
  assertFalse(validateMnemonic([...words.slice(0, 11), words[0]].join(" ")));
  assertFalse(validateMnemonic("not a passphrase"));
});

Deno.test("keys and node IDs match the original", async () => {
  const { bootstrap, keys } = await vectors(
    "dassie-identity/tests/data/keys.json",
  );
  for (const node of bootstrap) {
    assertEquals(nodeId(Uint8Array.fromHex(node.publicKey)), node.nodeId);
  }
  for (const key of keys) {
    const seed = Uint8Array.fromHex(key.seed);
    assertEquals(publicKey(seed).toHex(), key.publicKey);
    assertEquals(
      sign(seed, Uint8Array.fromHex(key.message)).toHex(),
      key.signature,
    );
  }
});

Deno.test("ILP packets match the original", async () => {
  const { packets } = await vectors("dassie-ilp/tests/data/vectors.json");
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
