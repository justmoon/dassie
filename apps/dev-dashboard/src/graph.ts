/**
 * The dashboard's network graph (spec 6 §2): a link between two nodes
 * only when each lists the other as its peer, and a link lit up for a
 * moment when a packet goes along it.
 */

import type { Link } from "@dassie/ui/network/nodes";

/** How long a link stays lit after a packet. */
export const PULSE_MS = 600;

/** The links of mutual peerings, each once, sorted. */
export function mutualLinks(
  peers: Readonly<Record<string, readonly string[]>>,
): Link[] {
  const links = new Map<string, Link>();
  for (const [a, list] of Object.entries(peers)) {
    for (const b of list) {
      if (a === b || !peers[b]?.includes(a)) continue;
      const [x, y] = a < b ? [a, b] : [b, a];
      links.set(`${x} ${y}`, { a: x, b: y });
    }
  }
  return [...links.values()].sort((p, q) =>
    p.a === q.a ? (p.b < q.b ? -1 : 1) : p.a < q.a ? -1 : 1
  );
}

/** The key of the link between `a` and `b`, either way round. */
export function linkKey(a: string, b: string): string {
  return a < b ? `${a} ${b}` : `${b} ${a}`;
}

/** Lit links: when each goes dark, by key. */
export class Pulses {
  readonly #until = new Map<string, number>();

  /** Lights the link between `from` and `to` from `now`. */
  hit(from: string, to: string, now: number): void {
    this.#until.set(linkKey(from, to), now + PULSE_MS);
  }

  /** Drops links gone dark; whether any did. */
  expire(now: number): boolean {
    let changed = false;
    for (const [key, until] of this.#until) {
      if (until <= now) {
        this.#until.delete(key);
        changed = true;
      }
    }
    return changed;
  }

  /** The keys lit now. */
  lit(): ReadonlySet<string> {
    return new Set(this.#until.keys());
  }
}
