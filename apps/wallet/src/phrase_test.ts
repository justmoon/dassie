import { test } from "node:test";
import { assert, assertEquals, assertMatch, assertThrows } from "@std/assert";
import { phraseFromSecret, phraseWords, readPhrase } from "@dassie/wasm";
import {
  carryMarks,
  clockOfDate,
  keyLetters,
  namesWord,
  readEntry,
  realmOf,
  typedTokens,
  typedWords,
} from "./phrase.ts";

const identity = { phraseWords, readPhrase };

/** 2026-10-27T12:00:00Z: the reader's clock in these tests. */
const NOW_MS = Date.UTC(2026, 9, 27, 12);
/** The hour of 2026-10-20, a week before. */
const LAST_WEEK_HOURS = (Date.UTC(2026, 9, 20) - Date.UTC(2026, 0, 1)) /
  3_600_000;

/** The words of a phrase of `realm` made last week. */
function made(realm: "test" | "live", byte = 7, hours = LAST_WEEK_HOURS) {
  const phrase = phraseFromSecret(new Uint8Array(16).fill(byte), realm, hours);
  try {
    return { text: phrase.text, words: phrase.words };
  } finally {
    phrase.free();
  }
}

/** `words` with the word at each of `positions` swapped for another. */
function wrong(words: readonly string[], ...positions: number[]): string {
  return words
    .map((word, index) =>
      positions.includes(index) ? (word === "zoo" ? "abandon" : "zoo") : word
    )
    .join(" ");
}

test("the node's realm comes from its ILP address", () => {
  assertEquals(realmOf("test"), "test");
  assertEquals(realmOf("g"), "live");
  assertEquals(realmOf(""), undefined);
  assertEquals(realmOf("example"), undefined);
});

test("a plate keeps a word's first four letters", () => {
  assertEquals(keyLetters("abandon"), ["aban", "don"]);
  assertEquals(keyLetters("zoo"), ["zoo", ""]);
});

test("the node's clock is its Date header", () => {
  assertEquals(
    clockOfDate("Tue, 27 Oct 2026 12:00:00 GMT"),
    NOW_MS,
  );
  assertEquals(clockOfDate(null), undefined);
  assertEquals(clockOfDate("soon"), undefined);
});

test("words are matched by their first four letters", () => {
  assert(namesWord(identity, "aban", "abandon"));
  assert(namesWord(identity, "ABANDON", "abandon"));
  assert(namesWord(identity, " acti ", "action"));
  assert(!namesWord(identity, "act", "action"));
  assert(namesWord(identity, "act", "act"));
  assert(!namesWord(identity, "abandon ability", "abandon"));
  assert(!namesWord(identity, "", "abandon"));
  assertEquals(typedWords(identity, "Aban ? zzzz  zoo"), [
    "abandon",
    undefined,
    undefined,
    "zoo",
  ]);
  assertEquals(typedWords(identity, "aban ".repeat(300)), undefined);
});

test("a phrase as written reads in canonical form", () => {
  const { text, words } = made("test");
  const sloppy = words.map((word) => word.slice(0, 4).toUpperCase())
    .join("   ");
  for (const typed of [text, sloppy, `\n${text}\n`]) {
    assertEquals(readEntry(identity, typed, [], "test", NOW_MS), {
      kind: "phrase",
      text,
    });
  }
});

test("a correction shows the changed words and is used only once confirmed", () => {
  const { text, words } = made("test");
  const cases: [string, number[], number[]][] = [
    // Two wrong words.
    [wrong(words, 3, 17), [], [3, 17]],
    // A word typed as ?, and one marked unreadable.
    [words.map((word, i) => (i === 0 ? "?" : word)).join(" "), [9], [0, 9]],
    // Five unreadable words, the most the code restores.
    [text, [1, 2, 3, 4, 5], [1, 2, 3, 4, 5]],
  ];
  for (const [typed, unreadable, changed] of cases) {
    const entry = readEntry(identity, typed, unreadable, "test", NOW_MS);
    assert(entry.kind === "corrected", typed);
    const { correction } = entry;
    assertEquals(correction.words, words);
    assertEquals([...correction.changed].sort((a, b) => a - b), changed);
    assertEquals(correction.date, "2026-10-20");
    assertEquals(correction.confirm(), text);
    assertThrows(() => correction.confirm(), Error, "spent");
    // Dismissing a spent correction does nothing.
    correction.dismiss();
  }
  const entry = readEntry(identity, wrong(words, 5), [], "test", NOW_MS);
  assert(entry.kind === "corrected");
  entry.correction.dismiss();
  assertThrows(() => entry.correction.confirm(), Error, "spent");
});

test("each refusal says what is wrong, before any correction", () => {
  const { text, words } = made("test");
  const refusal = (typed: string, realm: "test" | "live" = "test") => {
    const entry = readEntry(identity, typed, [], realm, NOW_MS);
    assert(entry.kind === "refused", typed);
    return entry.message;
  };
  assertMatch(refusal(words.slice(0, 12).join(" ")), /12-word phrase/);
  assertMatch(refusal(words.slice(0, 19).join(" ")), /this has 19/);
  assertMatch(refusal(wrong(words, 1, 2, 3)), /Too many words/);
  assertMatch(
    refusal(text, "live"),
    /for the testnet, and this node runs on the live network/,
  );
  // The realm is refused before the wrong word is corrected.
  assertMatch(refusal(wrong(made("live").words, 4)), /for the live network/);
  const tomorrow = made("test", 7, LAST_WEEK_HOURS + 7 * 24 + 37).text;
  assertMatch(refusal(tomorrow), /made on 2026-10-28.*past 2026-10-28/);
  assertMatch(refusal("aban ".repeat(300)), /longer than a phrase/);
});

// A mark is the owner's on a word, not on a place: an edit elsewhere moves
// it with its word, and it goes with the word or the text it was on.
test("marks on unreadable words stay on their words", () => {
  const carry = (before: string, after: string, marks: number[]) =>
    carryMarks(typedTokens(before), typedTokens(after), marks);
  assertEquals(typedTokens("  aban  abil\nable "), ["aban", "abil", "able"]);
  // A word inserted before the marked one moves the mark along.
  assertEquals(carry("a b d e", "a b c d e", [3]), [4]);
  // A word removed before it moves the mark back.
  assertEquals(carry("a b c d e", "a b d e", [0, 4]), [0, 3]);
  // Words typed after it leave it where it is.
  assertEquals(carry("a b c", "a b c d", [1]), [1]);
  // Editing the marked word, or removing it, drops its mark.
  assertEquals(carry("a b c", "a bx c", [1, 2]), [2]);
  assertEquals(carry("a b c", "a c", [1]), []);
  // Clearing the field drops every mark, stale ones too.
  assertEquals(carry("a b c", "", [0, 2, 7]), []);
  assertEquals(carry("a b c", "a b c", [0, 7]), [0]);
});
