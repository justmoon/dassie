// The golden vectors through the WASM builds: the bytes must be the ones
// the native tests check (crates/*/tests/data), so the wallet derives
// exactly what the node derives.

import { test } from "node:test";
import { readFile } from "node:fs/promises";

import { assert, assertEquals, assertThrows } from "@std/assert";

import {
  decodeIlpPacket,
  deriveNodeKey,
  encodeFulfill,
  encodePrepare,
  encodeReject,
  generatePhrase,
  isPhraseError,
  loginSecret,
  nodeId,
  type PhraseErrorCode,
  phraseFromSecret,
  publicKey,
  readPhrase,
  sign,
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

/** A reader's clock past every phrase's creation time. */
const AFTER_EVERY_PHRASE = Date.UTC(2145, 7, 16);

test("the passphrase chain matches the vectors", async () => {
  const { chain } = await vectors("chain.json");
  assert(chain.length > 60);
  for (const entry of chain) {
    const made = phraseFromSecret(
      Uint8Array.fromHex(entry.secret),
      entry.realm,
      entry.created,
    );
    assertEquals(made.text, entry.phrase);
    made.free();
    const nodeKey = deriveNodeKey(
      entry.phrase,
      entry.realm,
      AFTER_EVERY_PHRASE,
    );
    assertEquals(nodeKey.toHex(), entry.nodeKey);
    assertEquals(publicKey(nodeKey).toHex(), entry.publicKey);
    assertEquals(nodeId(publicKey(nodeKey)), entry.nodeId);
    assertEquals(loginSecret(nodeKey).toHex(), entry.loginSecret);
  }
});

/** The code a reading's refusal is thrown with, by the vectors' names. */
const CODES: Record<string, PhraseErrorCode> = {
  bip39: "bip39",
  length: "wordCount",
  tooDamaged: "tooDamaged",
  kindZero: "invalidKind",
  unassignedKind: "newerKind",
  otherRealm: "otherRealm",
  createdInFuture: "createdInFuture",
};

test("phrases read as the vectors say", async () => {
  const { made, reading } = await vectors("phrase.json");
  for (const entry of made) {
    const phrase = phraseFromSecret(
      Uint8Array.fromHex(entry.secret),
      entry.realm,
      entry.created,
    );
    assertEquals(phrase.words, entry.words);
    assertEquals(phrase.createdDate, entry.date);
    phrase.free();
  }
  for (const vector of reading) {
    // A word marked unreadable is typed as one that reads fine, so only the
    // mark makes it an erasure.
    const unreadable = vector.input.flatMap((word: string | null, at: number) =>
      word === null ? [at] : []
    );
    const text = vector.input.map((word: string | null) => word ?? "abandon")
      .join(" ");
    const read = () =>
      readPhrase(
        text,
        vector.realm,
        vector.clockUnixMs,
        Uint32Array.from(unreadable),
      );
    const { result } = vector;
    if (result.refused === undefined) {
      const reading = read();
      assertEquals(reading.corrected, result.corrected, vector.name);
      assertEquals([...reading.changed], result.changed, vector.name);
      assertEquals(reading.text, result.phrase, vector.name);
      assertEquals(reading.createdDate, result.date, vector.name);
      assertEquals(reading.realm, result.realm, vector.name);
      const phrase = reading.confirm();
      assertEquals(phrase.text, result.phrase);
      phrase.free();
      if (!result.corrected) {
        assertEquals(
          deriveNodeKey(text, vector.realm, vector.clockUnixMs).length,
          32,
        );
      } else {
        // A script has no one to confirm a correction, so it is refused,
        // by the positions alone.
        const error = assertThrows(() => read().exact());
        assert(isPhraseError(error));
        assertEquals(error.code, "needsConfirmation");
        assertEquals([...error.changed!], result.changed);
      }
      continue;
    }
    const error = assertThrows(read);
    assert(isPhraseError(error), vector.name);
    assertEquals(error.code, CODES[result.refused], vector.name);
    if (result.refused === "otherRealm") {
      assertEquals(error.realm, result.realm);
    }
    if (result.refused === "createdInFuture") {
      assertEquals(error.date, result.date);
    }
  }
});

test("a new phrase reads back by the clock that made it", () => {
  const now = Date.UTC(2026, 9, 27, 13, 45);
  const made = generatePhrase("live", now, now + 3_600_000);
  assertEquals(made.words.length, 20);
  assertEquals(made.createdDate, "2026-10-27");
  const reading = readPhrase(made.text.toUpperCase(), "live", now);
  assertEquals(reading.corrected, false);
  assertEquals(reading.confirm().nodeKey("live"), made.nodeKey("live"));
  const error = assertThrows(() => made.nodeKey("test"));
  assert(isPhraseError(error));
  assertEquals(error.code, "otherRealm");
  // No phrase is made while the wallet's clock and the node's are more
  // than a day apart, nor outside the hours a phrase records.
  const apart = assertThrows(() =>
    generatePhrase("test", now, now - 86_400_001)
  );
  assert(isPhraseError(apart));
  assertEquals(apart.code, "clocksApart");
  const early = assertThrows(() => generatePhrase("test", Date.UTC(2025, 0)));
  assert(isPhraseError(early));
  assertEquals(early.code, "clockOutOfRange");
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
