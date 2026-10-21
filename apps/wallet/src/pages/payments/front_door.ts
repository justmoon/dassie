/**
 * The node's name at the default front door, `<label>.dassiepay.net`, as a
 * node without a hostname is named (spec/areas/payments.md#the-nodes-names).
 * The front door reaches the node only while the node holds its connection
 * to it, as its relay, which it keeps by default (D40).
 */

/** The front door's domain. */
const DOMAIN = "dassiepay.net";

/** Whether a payment pointer names the node at the default front door. */
export function atFrontDoor(pointer: string): boolean {
  const host = pointer.replace(/^\$/, "").split("/")[0].split(":")[0];
  return host.toLowerCase().endsWith(`.${DOMAIN}`);
}

/** The relay the node reports (`GetStatus`). */
export interface Relay {
  readonly url: string;
  readonly connected: boolean;
}

/**
 * What the wallet says of a name at the default front door:
 * - `reachable`: the node holds its connection to the front door;
 * - `disconnected`: the front door is its relay, but not connected now;
 * - `other-relay`: the node keeps another relay, so the front door does not
 *   reach it;
 * - `no-relay`: the node keeps none (`DASSIE_RELAY` is empty);
 * - `unknown`: the node's status is not known yet.
 */
export type FrontDoorState =
  | "reachable"
  | "disconnected"
  | "other-relay"
  | "no-relay"
  | "unknown";

/**
 * What to say of `pointer`, given the relay the node reports, or `null` when
 * it reports none, or `undefined` while its status is not known; `undefined`
 * when the pointer is not at the front door.
 */
export function frontDoorState(
  pointer: string | undefined,
  relay: Relay | null | undefined,
): FrontDoorState | undefined {
  if (pointer === undefined || !atFrontDoor(pointer)) return undefined;
  if (relay === undefined) return "unknown";
  if (relay === null) return "no-relay";
  if (!isFrontDoor(relay.url)) return "other-relay";
  return relay.connected ? "reachable" : "disconnected";
}

/** Whether a relay's URL is the default front door's. */
function isFrontDoor(url: string): boolean {
  try {
    const host = new URL(url).hostname.toLowerCase();
    return host === DOMAIN || host.endsWith(`.${DOMAIN}`);
  } catch {
    return false;
  }
}
