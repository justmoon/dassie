/**
 * A small force-directed layout for the network graph: nodes repel each
 * other, links pull their ends together, and everything drifts to the
 * center. It is deterministic: a node starts where its ID puts it, so the
 * same table always settles the same way, and nodes keep their place as
 * the table changes.
 *
 * The network graph has at most a few hundred nodes, so the repulsion's
 * O(n²) costs nothing worth a quadtree.
 */

import { seedIndex } from "./nodes.ts";

/** A node's place and speed. */
export interface Body {
  x: number;
  y: number;
  vx: number;
  vy: number;
}

const REPULSION = 900;
const SPRING_LENGTH = 70;
const SPRING = 0.04;
const GRAVITY = 0.01;
const DAMPING = 0.6;

/** The layout's state: where each node is, and how lively it still is. */
export class Layout {
  readonly bodies = new Map<string, Body>();
  #alpha = 1;

  /** How lively it is; it stops moving below 0.005. */
  get alpha(): number {
    return this.#alpha;
  }

  /** Whether it has settled. */
  get settled(): boolean {
    return this.#alpha < 0.005;
  }

  /** Takes the current node IDs: new ones get a place, gone ones go. */
  update(ids: readonly string[]): void {
    const wanted = new Set(ids);
    let changed = false;
    for (const id of [...this.bodies.keys()]) {
      if (!wanted.has(id)) {
        this.bodies.delete(id);
        changed = true;
      }
    }
    for (const id of ids) {
      if (this.bodies.has(id)) continue;
      const angle = (seedIndex(id, 360) / 360) * 2 * Math.PI;
      const radius = 40 + seedIndex(`${id}r`, 120);
      this.bodies.set(id, {
        x: Math.cos(angle) * radius,
        y: Math.sin(angle) * radius,
        vx: 0,
        vy: 0,
      });
      changed = true;
    }
    if (changed) this.#alpha = Math.max(this.#alpha, 0.5);
  }

  /** One step of the simulation over `links`. */
  tick(links: readonly { a: string; b: string }[]): void {
    if (this.settled) return;
    const bodies = [...this.bodies.values()];
    for (let i = 0; i < bodies.length; i++) {
      for (let j = i + 1; j < bodies.length; j++) {
        const [p, q] = [bodies[i], bodies[j]];
        const dx = q.x - p.x || 0.01;
        const dy = q.y - p.y || 0.01;
        const distance2 = Math.max(dx * dx + dy * dy, 25);
        const force = (REPULSION / distance2) * this.#alpha;
        const distance = Math.sqrt(distance2);
        const [fx, fy] = [(dx / distance) * force, (dy / distance) * force];
        p.vx -= fx;
        p.vy -= fy;
        q.vx += fx;
        q.vy += fy;
      }
    }
    for (const { a, b } of links) {
      const p = this.bodies.get(a);
      const q = this.bodies.get(b);
      if (!p || !q) continue;
      const dx = q.x - p.x;
      const dy = q.y - p.y;
      const distance = Math.sqrt(dx * dx + dy * dy) || 1;
      const pull = (distance - SPRING_LENGTH) * SPRING * this.#alpha;
      const [fx, fy] = [(dx / distance) * pull, (dy / distance) * pull];
      p.vx += fx;
      p.vy += fy;
      q.vx -= fx;
      q.vy -= fy;
    }
    for (const body of bodies) {
      body.vx = (body.vx - body.x * GRAVITY * this.#alpha) * DAMPING;
      body.vy = (body.vy - body.y * GRAVITY * this.#alpha) * DAMPING;
      body.x += body.vx;
      body.y += body.vy;
    }
    this.#alpha *= 0.97;
  }

  /** Runs `steps` ticks at once, for a still picture. */
  settle(links: readonly { a: string; b: string }[], steps = 300): void {
    for (let step = 0; step < steps && !this.settled; step++) this.tick(links);
  }

  /** The smallest box around every node, with `margin`. */
  bounds(margin = 30): { x: number; y: number; width: number; height: number } {
    const bodies = [...this.bodies.values()];
    if (bodies.length === 0) {
      return { x: -100, y: -100, width: 200, height: 200 };
    }
    const xs = bodies.map((b) => b.x);
    const ys = bodies.map((b) => b.y);
    const [minX, maxX] = [Math.min(...xs), Math.max(...xs)];
    const [minY, maxY] = [Math.min(...ys), Math.max(...ys)];
    return {
      x: minX - margin,
      y: minY - margin,
      width: Math.max(maxX - minX + 2 * margin, 100),
      height: Math.max(maxY - minY + 2 * margin, 100),
    };
  }
}
