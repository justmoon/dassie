/**
 * The last few payments, in and out together, as home lists them: from the
 * same streams as Activity, each asked for its newest few.
 */

/** The fields the order reads. */
interface Dated {
  id: string;
  createdAtUnixMs: bigint;
}

/** A payment either way, with a key that tells the two lists apart. */
export type RecentPayment<In, Out> =
  | { key: string; direction: "in"; payment: In }
  | { key: string; direction: "out"; payment: Out };

/** The newest `count` of both lists, newest first; ties by key, so the
 * order never jumps. */
export function recent<In extends Dated, Out extends Dated>(
  incoming: readonly In[],
  outgoing: readonly Out[],
  count: number,
): RecentPayment<In, Out>[] {
  const all: RecentPayment<In, Out>[] = [
    ...incoming.map((payment) => ({
      key: `in-${payment.id}`,
      direction: "in" as const,
      payment,
    })),
    ...outgoing.map((payment) => ({
      key: `out-${payment.id}`,
      direction: "out" as const,
      payment,
    })),
  ];
  return all.sort((a, b) =>
    a.payment.createdAtUnixMs === b.payment.createdAtUnixMs
      ? a.key.localeCompare(b.key)
      : a.payment.createdAtUnixMs > b.payment.createdAtUnixMs
      ? -1
      : 1
  ).slice(0, count);
}
