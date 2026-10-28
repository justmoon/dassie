/**
 * Incoming payments as the wallet shows them: where each is, who asked for
 * it, and what for.
 */

/** Where an incoming payment is. */
export type IncomingState = "open" | "completed" | "expired";

/** The fields these helpers read. */
export interface IncomingFields {
  id: string;
  client: string;
  metadataJson: string;
  createdAtUnixMs: bigint;
  expiresAtUnixMs: bigint;
  completedAtUnixMs?: bigint;
}

/** Whether it still takes money at `nowMs`. */
export function incomingState(
  payment: Pick<IncomingFields, "completedAtUnixMs" | "expiresAtUnixMs">,
  nowMs: number,
): IncomingState {
  if (payment.completedAtUnixMs !== undefined) return "completed";
  return BigInt(nowMs) < payment.expiresAtUnixMs ? "open" : "expired";
}

/** What it shows it received: its amount once that is paid in full, as
 * the node counts what lands within a tolerance of it, under or over
 * (spec/decisions/paid-within-a-tolerance.md); else what arrived. */
export function shownReceived(
  payment: {
    incomingAmount?: bigint;
    receivedAmount: bigint;
    paidInFull: boolean;
  },
): bigint {
  return payment.paidInFull && payment.incomingAmount !== undefined
    ? payment.incomingAmount
    : payment.receivedAmount;
}

/** Newest first; ties by ID, so the order never jumps. */
export function newestFirst<T extends IncomingFields>(
  payments: readonly T[],
): T[] {
  return [...payments].sort((a, b) =>
    a.createdAtUnixMs === b.createdAtUnixMs
      ? a.id.localeCompare(b.id)
      : a.createdAtUnixMs > b.createdAtUnixMs
      ? -1
      : 1
  );
}

/** Who created it: the owner, a client by wallet address, or by key. */
export function clientLabel(client: string): string {
  if (client === "") return "You";
  if (client.startsWith("jwk:")) return "A client known by its key";
  try {
    const url = new URL(client);
    return `${url.host}${url.pathname === "/" ? "" : url.pathname}`;
  } catch {
    return client;
  }
}

/** What its creator said it is for, if anything. */
export function description(metadataJson: string): string | undefined {
  if (metadataJson === "") return undefined;
  try {
    const metadata: unknown = JSON.parse(metadataJson);
    if (typeof metadata !== "object" || metadata === null) return undefined;
    const text = (metadata as Record<string, unknown>).description;
    return typeof text === "string" && text !== "" ? text : undefined;
  } catch {
    return undefined;
  }
}
