import { test } from "node:test";
import { assertEquals, assertMatch, assertNotMatch } from "@std/assert";
import type { PhraseError, PhraseErrorCode } from "@dassie/wasm/web";
import { phraseNotMade, phraseRefusal } from "./messages.ts";

function refusal(
  code: PhraseErrorCode,
  details: Partial<PhraseError> = {},
): PhraseError {
  return Object.assign(new Error(code), { code }, details);
}

// The decision's refusals each get words of their own, so the owner can
// tell a phrase of another wallet from a damaged one, a newer one or one of
// the other network.
test("each refusal of a phrase reads differently", () => {
  const messages = [
    phraseRefusal(refusal("bip39", { words: 24 }), "test"),
    phraseRefusal(refusal("wordCount", { words: 19 }), "test"),
    phraseRefusal(refusal("tooDamaged"), "test"),
    phraseRefusal(refusal("invalidKind"), "test"),
    phraseRefusal(refusal("newerKind", { kind: 3 }), "test"),
    phraseRefusal(refusal("otherRealm", { realm: "live" }), "test"),
    phraseRefusal(
      refusal("createdInFuture", { date: "2026-11-02" }),
      "test",
    ),
    phraseRefusal(refusal("tooLong"), "test"),
    phraseRefusal(refusal("clockOutOfRange"), "test"),
  ];
  assertEquals(new Set(messages).size, messages.length);
  assertMatch(messages[0], /24-word phrase.*another wallet/);
  assertMatch(messages[1], /20 words, and this has 19/);
  assertMatch(messages[4], /newer version of Dassie/);
  assertMatch(messages[5], /for the live network.*on the testnet/);
  assertMatch(messages[6], /2026-11-02.*clock.*past 2026-11-02/);
  for (const message of messages) assertNotMatch(message, /undefined/);
});

test("a refusal without its details still reads", () => {
  assertMatch(
    phraseRefusal(refusal("bip39"), "live"),
    /^That is a BIP-39 phrase/,
  );
  assertMatch(
    phraseRefusal(refusal("otherRealm"), "live"),
    /for the testnet, and this node runs on the live network/,
  );
  assertNotMatch(phraseRefusal(refusal("wordCount"), "test"), /undefined/);
});

test("no phrase is made while the clocks disagree", () => {
  assertMatch(phraseNotMade(refusal("clocksApart")), /more than a day apart/);
  assertMatch(phraseNotMade(refusal("clockOutOfRange")), /Set it right/);
});
