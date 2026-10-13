/** What the wallet tells people when something goes wrong. */

import type { Outcome } from "./auth.ts";

/** The passphrase is not a BIP-39 phrase (wrong word, typo, bad checksum). */
export const INVALID_PASSPHRASE = "That is not a valid passphrase.";

/** The key derivation module failed to load. */
export const IDENTITY_FAILED =
  "The wallet could not load its key derivation module. Reload the page to try again.";

const UNREACHABLE =
  "The node could not be reached. Check your connection and try again.";

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
