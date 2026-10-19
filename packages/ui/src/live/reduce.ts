/**
 * Live collections: a snapshot, then upserts and removals by key
 * (spec/areas/rpc.md#live-streams). The reducers change a plain draft, so
 * the same code runs inside a Solid store's setter and in tests.
 */

/** A collection by key, and whether its first snapshot arrived. */
export interface Keyed<T> {
  ready: boolean;
  byId: Record<string, T>;
}

/** An empty collection. */
export function emptyKeyed<T>(): Keyed<T> {
  return { ready: false, byId: {} };
}

/**
 * Applies one item of a snapshot-and-delta stream. An item with `reset`
 * starts the collection over; the items after it (the rest of a large
 * snapshot, then changes) upsert and remove.
 */
export function applyTableEvent<T>(
  draft: Keyed<T>,
  event: { reset: boolean; upserted: readonly T[]; removed: readonly string[] },
  key: (entry: T) => string,
): void {
  if (event.reset) {
    for (const id of Object.keys(draft.byId)) delete draft.byId[id];
    draft.ready = true;
  }
  for (const entry of event.upserted) draft.byId[key(entry)] = entry;
  for (const id of event.removed) delete draft.byId[id];
}

/** Appends `lines` to a ring of at most `max`; returns how many it dropped. */
export function pushRing<T>(
  ring: T[],
  lines: readonly T[],
  max: number,
): number {
  ring.push(...lines);
  const excess = Math.max(0, ring.length - max);
  if (excess > 0) ring.splice(0, excess);
  return excess;
}
