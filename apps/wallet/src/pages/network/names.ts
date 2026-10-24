/**
 * What the names page says of the owner's names
 * (spec/areas/network.md#the-registrant): how each stands, and how it
 * stands with each registrar, as the node reports it.
 */

import { type NameAtRegistrar, NameState } from "@dassie/rpc/proto";
import { relativeTime } from "@dassie/ui/format";

/** How a name stands, in a few words, and its badge's tone. */
export function nameState(state: NameState): {
  text: string;
  tone: "good" | "plain" | "bad";
} {
  switch (state) {
    case NameState.COMMITTING:
      return { text: "committing", tone: "plain" };
    case NameState.UNSEALING:
      return { text: "unsealing", tone: "plain" };
    case NameState.WAITING:
      return { text: "waiting for the registrars", tone: "plain" };
    case NameState.HELD:
      return { text: "yours", tone: "good" };
    case NameState.LAPSING:
      return { text: "renewing late", tone: "bad" };
    case NameState.LOST:
      return { text: "taken by another node", tone: "bad" };
    case NameState.SPLIT:
      return { text: "split between nodes", tone: "bad" };
    case NameState.NOT_HELD:
      return { text: "not granted", tone: "bad" };
    case NameState.ABANDONED:
      return { text: "not every registrar answered", tone: "bad" };
    default:
      return { text: "unknown", tone: "plain" };
  }
}

/** What a name in this state means for the owner, if anything. */
export function nameHint(state: NameState): string | undefined {
  switch (state) {
    case NameState.COMMITTING:
      return "Your node is committing to the name, sealed, at every registrar. No registrar learns the name until every one of them has logged the commitment.";
    case NameState.UNSEALING:
    case NameState.WAITING:
      return "Every registrar logged the commitment, and your node told each the name. It is yours once their windows close, unless someone committed to it before you.";
    case NameState.LAPSING:
      return "Too few registrars list it now. Your node renews it at the others while they keep it for you.";
    case NameState.LOST:
      return "Someone committed to it before you did.";
    case NameState.SPLIT:
      return "Someone committed to it at about the same time, and the registrars disagree. Nobody holds it; once they release it, you can register it again.";
    case NameState.NOT_HELD:
    case NameState.ABANDONED:
      return "You can register it again.";
    default:
      return undefined;
  }
}

/** Whether the owner can renew a name in this state. */
export function renewable(state: NameState): boolean {
  return state === NameState.HELD || state === NameState.LAPSING;
}

/** Whether the owner can register a name in this state again. */
export function registrable(state: NameState): boolean {
  return state === NameState.NOT_HELD || state === NameState.ABANDONED ||
    state === NameState.SPLIT;
}

/** A refusal's status, in words. */
function refusal(status: number): string {
  switch (status) {
    case 409:
      return "refused";
    case 429:
      return "busy, asking again";
    case 503:
      return "full, asking again";
    default:
      return `refused (${status})`;
  }
}

/**
 * How a name stands with one registrar: what it logged and took of this
 * node's, and what its list says.
 */
export function atRegistrar(
  at: NameAtRegistrar,
  node: string,
  nowMs: number,
): string {
  const parts: string[] = [];
  if (at.unsealed) parts.push("took the unsealing");
  else if (at.receipt) parts.push("logged the commitment");
  if (at.refusedStatus !== 0) parts.push(refusal(at.refusedStatus));
  if (at.listedHolder === node) {
    const expiry = relativeTime(at.listedExpiresAtUnixMs, nowMs);
    parts.push(`lists it for you (expiry ${expiry})`);
  } else if (at.listedHolder !== "") {
    parts.push("lists another node");
  }
  if (!at.listFresh) parts.push("its list is not current");
  return parts.length === 0 ? "nothing yet" : parts.join(", ");
}
