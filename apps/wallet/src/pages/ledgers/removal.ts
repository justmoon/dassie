/**
 * Removing a ledger (spec/areas/wallet.md#remove-a-ledger): what must hold
 * first, as the node reports it, and the confirmation the owner types.
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
 * The checklist: always whether peers settle with the scheme, as the
 * original showed, and each other blocker the node names.
 */
export function removalChecklist(
  blockers: readonly LedgerRemovalBlocker[],
): CheckItem[] {
  const peers = blockers.find((blocker) => blocker.kind === Kind.PEERS);
  const items: CheckItem[] = [{
    id: "peers",
    label: peers
      ? `Settlement scheme is still in use for ${
        plural(peers.count, "peer", "peers")
      }`
      : "Not used by any peers",
    state: peers ? "invalid" : "valid",
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
