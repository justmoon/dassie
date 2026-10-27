/**
 * Removing a ledger (spec/areas/wallet.md#pages): what must hold
 * first, as the node reports it, the confirmation the owner types, and a
 * ledger whose peers the node settles up with before it goes
 * (spec/decisions/settlement-methods.md).
 */

import {
  type LedgerRemovalBlocker,
  LedgerRemovalBlocker_Kind as Kind,
} from "@dassie/rpc/proto";
import type { CheckItem } from "@dassie/ui/ui";

/** What the owner types to confirm, e.g. `DELETE STUB`. */
export function expectedConfirmation(scheme: string): string {
  return `DELETE ${scheme.toUpperCase()}`;
}

const plural = (count: number, one: string, many: string) =>
  `${count} ${count === 1 ? one : many}`;

/**
 * The checklist: always what happens to the peers that settle with the
 * scheme, which no longer keep it, and each blocker the node names.
 */
export function removalChecklist(
  blockers: readonly LedgerRemovalBlocker[],
): CheckItem[] {
  const items: CheckItem[] = [{
    id: "peers",
    label:
      "Peers that settle with it are asked to settle once, then your node stops settling with them on it",
    state: "valid",
  }];
  for (const blocker of blockers) {
    if (blocker.kind === Kind.BTP_TOKENS) {
      items.push({
        id: "btp-tokens",
        label: `${
          plural(blocker.count, "BTP token uses", "BTP tokens use")
        } its ledger; revoke ${blocker.count === 1 ? "it" : "them"} first`,
        state: "invalid",
      });
    } else if (blocker.kind === Kind.FINAL_SETTLEMENTS) {
      items.push({
        id: "final-settlements",
        label: `${
          plural(blocker.count, "ended peering", "ended peerings")
        } still to be settled with it`,
        state: "invalid",
      });
    } else if (blocker.kind === Kind.OUTGOING_PAYMENTS) {
      items.push({
        id: "outgoing-payments",
        label: `${
          plural(blocker.count, "payment from it is", "payments from it are")
        } still being sent`,
        state: "invalid",
      });
    } else if (blocker.kind === Kind.HUB_MOVE) {
      items.push({
        id: "hub-move",
        label:
          "Funds are on their way between its settlement hub and the ledger that funds it; wait for them, or give the move up on the Hubs page once it is late",
        state: "invalid",
      });
    } else if (blocker.kind !== Kind.PEERS) {
      items.push({
        id: `other-${blocker.kind}`,
        label: "Something else keeps it in use",
        state: "invalid",
      });
    }
  }
  return items;
}

/**
 * What the owner sees of a ledger being removed: how many peers the node
 * is still settling up with on it. Nothing for one not being removed.
 */
export function removingNotice(
  removingMethods: number | undefined,
): { title: string; text: string } | undefined {
  if (removingMethods === undefined) {
    return undefined;
  }
  return {
    title: "Being removed",
    text: removingMethods === 0
      ? "Your node removes it as soon as nothing else uses it."
      : `Your node is settling up with ${
        plural(removingMethods, "peer", "peers")
      } on it first, which takes a few minutes at most. Then it goes.`,
  };
}
