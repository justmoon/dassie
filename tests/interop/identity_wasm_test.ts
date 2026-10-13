// The golden vectors through the WASM builds: the bytes must be the ones
// the native tests check (crates/*/tests/data), so the wallet derives
// exactly what the node derives.

import { assert, assertEquals, assertFalse } from "@std/assert";
import { decodeHex, encodeHex } from "@std/encoding/hex";

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
  const document = JSON.parse(await Deno.readTextFile(new URL(path, crates)));
  assertEquals(document.source, "e962ab78");
  return document;
}

Deno.test("the passphrase chain matches the original", async () => {
  const { chain } = await vectors("dassie-identity/tests/data/chain.json");
  assert(chain.length > 60);
  for (const entry of chain) {
    assert(validateMnemonic(entry.mnemonic));
    const nodeKey = deriveNodeKey(entry.mnemonic);
    assertEquals(encodeHex(nodeKey), entry.nodeKey);
    assertEquals(encodeHex(publicKey(nodeKey)), entry.publicKey);
    assertEquals(nodeId(publicKey(nodeKey)), entry.nodeId);
    assertEquals(encodeHex(loginSecret(nodeKey)), entry.loginSecret);
    assertEquals(
      encodeHex(devSessionToken(entry.mnemonic)),
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
    assertEquals(nodeId(decodeHex(node.publicKey)), node.nodeId);
  }
  for (const key of keys) {
    const seed = decodeHex(key.seed);
    assertEquals(encodeHex(publicKey(seed)), key.publicKey);
    assertEquals(encodeHex(sign(seed, decodeHex(key.message))), key.signature);
  }
});

Deno.test("ILP packets match the original", async () => {
  const { packets } = await vectors("dassie-ilp/tests/data/vectors.json");
  for (const vector of packets) {
    const bytes = decodeHex(vector.encoded);
    const packet = decodeIlpPacket(bytes);
    const data = vector.packet.data;
    let encoded: Uint8Array;
    if (packet.type === 12) {
      assertEquals(packet.address, data.destination);
      encoded = encodePrepare(
        BigInt(data.amount),
        packet.expiresAt,
        decodeHex(data.executionCondition),
        data.destination,
        decodeHex(data.data),
      );
    } else if (packet.type === 13) {
      encoded = encodeFulfill(
        decodeHex(data.fulfillment),
        decodeHex(data.data),
      );
    } else {
      assertEquals(packet.code, data.code);
      encoded = encodeReject(
        data.code,
        data.triggeredBy,
        data.message,
        decodeHex(data.data),
      );
    }
    assertEquals(encodeHex(encoded), vector.encoded, vector.name);
  }
});
