/** What the wallet tells people when something goes wrong. */

import type { PhraseError } from "@dassie/wasm/web";
import type { Outcome } from "./auth.ts";
import type { Realm } from "./phrase.ts";

/** The key derivation module failed to load. */
export const IDENTITY_FAILED =
  "The wallet could not load its key derivation module. Reload the page to try again.";

/** The node's clock could not be read before making a phrase. */
export const NODE_CLOCK_UNKNOWN =
  "The wallet could not read your node's clock, which it compares with this device's before it makes a phrase. Check your connection and try again.";

/** The node reports a realm this wallet does not know. */
export const UNKNOWN_REALM =
  "Your node runs on a network this wallet does not know. Is the wallet older than the node?";

const UNREACHABLE =
  "The node could not be reached. Check your connection and try again.";

/** How the wallet names a realm to the owner. */
export function realmName(realm: Realm): string {
  return realm === "live" ? "the live network" : "the testnet";
}

/**
 * Why a phrase the owner entered was refused
 * (spec/decisions/phrase-format.md#decision), in terms of what they can do
 * about it. None of it names a word of the phrase.
 */
export function phraseRefusal(error: PhraseError, expected: Realm): string {
  switch (error.code) {
    case "bip39": {
      const phrase = error.words === undefined
        ? "a BIP-39 phrase"
        : `a ${error.words}-word phrase`;
      return `That is ${phrase}: another wallet's, or one from an earlier version of Dassie. A Dassie phrase has 20 words, and the older phrases can no longer be read.`;
    }
    case "wordCount":
      return `A phrase has 20 words, and this has ${
        error.words ?? "a different number"
      }. Type ? in place of a word you cannot read.`;
    case "tooLong":
      return "That is much longer than a phrase.";
    case "tooDamaged":
      return "Too many words are wrong or unreadable to restore the phrase. Check each word against your copy, and mark a word you cannot read rather than guess it: the wallet restores up to five words marked unreadable, but at most two wrong ones.";
    case "invalidKind":
      return "These words are not a Dassie phrase.";
    case "newerKind":
      return "This phrase was made by a newer version of Dassie than your node runs. Update the node, then try again.";
    case "otherRealm":
      return `This is a phrase for ${
        realmName(error.realm ?? (expected === "live" ? "test" : "live"))
      }, and this node runs on ${
        realmName(expected)
      }. Each network has a phrase of its own: enter the one for ${
        realmName(expected)
      }.`;
    case "createdInFuture": {
      const date = error.date ?? "a later date";
      return `This phrase says it was made on ${date}, more than a day after the time on this device's clock. A phrase cannot be made in the future: this device's clock is behind, the words are not your phrase, or the clock of the device that made it was ahead. In that last case, set this device's clock past ${date} to read the phrase.`;
    }
    case "clockOutOfRange":
      return "This device's clock shows a time no phrase can have. Set it right and try again.";
    default:
      return "The wallet could not read that phrase.";
  }
}

/** Why no phrase was made: the clocks it is checked against. */
export function phraseNotMade(error: PhraseError): string {
  switch (error.code) {
    case "clocksApart":
      return "This device's clock and your node's are more than a day apart. A phrase records when it was made, so the wallet makes one only once the two agree: set the wrong clock right and try again.";
    case "clockOutOfRange":
      return "This device's clock shows a time no phrase can record. Set it right and try again.";
    default:
      return "The wallet could not make a phrase.";
  }
}

type Failure = Extract<Outcome, { ok: false }>;

/** Why setup failed: the node's own words, which name the problem. */
export function setupFailure(failure: Failure): string {
  if (failure.status === 0) return UNREACHABLE;
  return failure.message || `Setup failed (HTTP ${failure.status}).`;
}

/** Why login failed, in terms of what the person can do about it. */
export function loginFailure(failure: Failure): string {
  switch (failure.status) {
    case 0:
      return UNREACHABLE;
    case 401:
      return "That passphrase does not belong to this node.";
    case 429:
      return failure.retryAfterSeconds === undefined
        ? "Too many attempts. Try again later."
        : `Too many attempts. Try again in ${failure.retryAfterSeconds} seconds.`;
    default:
      return failure.message
        ? `Could not sign in: ${failure.message}`
        : `Could not sign in (HTTP ${failure.status}). Please try again.`;
  }
}

/** Why logout failed. */
export function logoutFailure(failure: Failure): string {
  if (failure.status === 0) return UNREACHABLE;
  return failure.message
    ? `Could not log out: ${failure.message}`
    : `Could not log out (HTTP ${failure.status}).`;
}
