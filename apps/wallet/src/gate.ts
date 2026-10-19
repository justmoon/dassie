/**
 * Which screen the wallet shows (spec/areas/wallet.md#the-gate), from the
 * node's basic state, the path, and, once the owner is signed in, the live
 * ledgers.
 */

import { type GetBasicStateResponse, NodeState } from "@dassie/rpc/proto";

/** The token of a setup URL, `/setup/<64 hex digits>`. */
const SETUP_PATH = /^\/setup\/([0-9a-fA-F]{64})\/?$/;

/** A screen, before the owner signs in. */
export type Screen =
  | { kind: "loading" }
  | { kind: "unreachable"; message: string }
  | { kind: "setup"; token: string }
  | { kind: "setup-info" }
  | { kind: "login" }
  | { kind: "owner" }
  | { kind: "unknown" };

/** The screen for the node's basic state, or its failure. */
export function decideScreen(
  basic: GetBasicStateResponse | undefined,
  failure: string | undefined,
  pathname: string,
): Screen {
  if (failure !== undefined) return { kind: "unreachable", message: failure };
  if (!basic) return { kind: "loading" };
  switch (basic.state) {
    case NodeState.UNINITIALIZED: {
      const token = SETUP_PATH.exec(pathname)?.[1]?.toLowerCase();
      return token ? { kind: "setup", token } : { kind: "setup-info" };
    }
    case NodeState.ANONYMOUS:
      return { kind: "login" };
    case NodeState.AUTHENTICATED:
      return { kind: "owner" };
    default:
      return { kind: "unknown" };
  }
}

/**
 * What the signed-in owner sees: nothing until the ledgers are known, the
 * forced choice of a first ledger without one
 * (spec/areas/wallet.md#the-gate), the wallet otherwise. It follows
 * the live ledgers, so adding the first ledger, or removing the last, takes
 * effect at once (UI-7).
 */
export function ownerScreen(
  ledgers: readonly unknown[] | undefined,
): "loading" | "onboarding" | "wallet" {
  if (ledgers === undefined) return "loading";
  return ledgers.length === 0 ? "onboarding" : "wallet";
}

/** Whether the path is a setup URL, which a set-up node leaves. */
export function isSetupPath(pathname: string): boolean {
  return pathname.startsWith("/setup/");
}
