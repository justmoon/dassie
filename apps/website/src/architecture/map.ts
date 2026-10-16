/**
 * The architecture map's data (`map.json`) and what the page works out
 * from it: where the arrows run, and what a selection lights up. PRs that
 * change Dassie's architecture edit `map.json` by hand; `map_test.ts`
 * checks that it holds together.
 *
 * @module
 */

import data from "./map.json" with { type: "json" };

/** How one box relates to another: the arrow's dash pattern. */
export type EdgeKind = "uses" | "talks" | "wasm" | "runs";

/** A group of components that work together, with a colour of its own. */
export interface Subsystem {
  /** Names its colour class, `sub-<id>`, in `site.css`. */
  id: string;
  name: string;
  desc: string;
}

/** A crate, app or package: a box on the map. */
export interface Component {
  name: string;
  /** Where it lives in the repository. */
  path: string;
  label: string;
  /** The id of its subsystem. */
  sub: string;
  x: number;
  y: number;
  w: number;
  h?: number;
  desc: string;
  does: string[];
}

export interface Edge {
  /** From the box that builds on… */
  f: string;
  /** …to the box it builds on. */
  t: string;
  k: EdgeKind;
  /** Hidden unless a selection lights it or every link is shown. */
  h: boolean;
}

/** A journey: the boxes a task passes through, step by step. */
export interface Flow {
  id: string;
  button: string;
  title: string;
  steps: { ids: string[]; text: string }[];
}

export interface Release {
  version: string;
  /** YYYY-MM-DD. */
  date: string;
  changes: string[];
}

export interface ArchitectureMap {
  intro: string[];
  subsystems: Subsystem[];
  viewBox: { w: number; h: number };
  /** The layers behind the boxes. */
  bands: { t: string; s: string; x: number; y: number; w: number; h: number }[];
  notes: {
    text: string;
    x: number;
    y: number;
    anchor?: "start" | "middle" | "end";
  }[];
  components: Record<string, Component>;
  edges: Edge[];
  flows: Flow[];
  testsIntro: string;
  tests: { t: string; p: string; ev: string }[];
  /** Newest first. */
  releases: Release[];
}

export const MAP = data as ArchitectureMap;

/** A box's height when the data gives none. */
export const BOX_HEIGHT = 60;

/** The class that colours a box, swatch or chip by its subsystem. */
export function subsystemClass(id: string): string {
  return `sub-${id}`;
}

export type Selection =
  | { type: "none" }
  | { type: "component"; id: string }
  | { type: "flow"; id: string };

/** What a selection lights up. `boxes` is null when nothing is selected. */
export interface Highlight {
  boxes: Set<string> | null;
  /** Indexes into `map.edges`. */
  edges: Set<number>;
  /** The journey step at which each box first appears. */
  badges: Map<string, number>;
}

/**
 * The boxes and edges `selection` lights: a component with every link it
 * has, hidden or not; a journey with the visible links among its boxes.
 */
export function highlight(
  map: ArchitectureMap,
  selection: Selection,
): Highlight {
  const edges = new Set<number>();
  const badges = new Map<string, number>();
  if (selection.type === "none") return { boxes: null, edges, badges };
  const boxes = new Set<string>();
  if (selection.type === "component") {
    boxes.add(selection.id);
    map.edges.forEach((edge, index) => {
      if (edge.f !== selection.id && edge.t !== selection.id) return;
      edges.add(index);
      boxes.add(edge.f);
      boxes.add(edge.t);
    });
    return { boxes, edges, badges };
  }
  flow(map, selection.id)?.steps.forEach((step, index) => {
    for (const id of step.ids) {
      boxes.add(id);
      if (!badges.has(id)) badges.set(id, index + 1);
    }
  });
  map.edges.forEach((edge, index) => {
    if (!edge.h && boxes.has(edge.f) && boxes.has(edge.t)) edges.add(index);
  });
  return { boxes, edges, badges };
}

/** The journey `id`, if the map has it. */
export function flow(map: ArchitectureMap, id: string): Flow | undefined {
  return map.flows.find((candidate) => candidate.id === id);
}

/** The subsystem `id`, if the map has it. */
export function subsystem(
  map: ArchitectureMap,
  id: string,
): Subsystem | undefined {
  return map.subsystems.find((candidate) => candidate.id === id);
}

interface Box {
  x: number;
  y: number;
  w: number;
  h?: number;
}

const clamp = (value: number, low: number, high: number) =>
  Math.max(low, Math.min(high, value));

/**
 * The SVG path of an arrow from box `a` to box `b`: straight across when
 * they sit side by side, otherwise a curve from one's bottom or top edge
 * to the other's. A wide box (a layer, like `node`) gives the arrow its
 * end near the other box rather than at its own centre.
 */
export function edgePath(a: Box, b: Box): string {
  const ah = a.h ?? BOX_HEIGHT, bh = b.h ?? BOX_HEIGHT;
  const acx = a.x + a.w / 2, bcx = b.x + b.w / 2;
  const acy = a.y + ah / 2, bcy = b.y + bh / 2;
  const top = Math.max(a.y, b.y), bottom = Math.min(a.y + ah, b.y + bh);
  if (bottom - top > 20) {
    const right = bcx > acx, y = (top + bottom) / 2;
    return `M${right ? a.x + a.w : a.x},${y} L${right ? b.x : b.x + b.w},${y}`;
  }
  const down = bcy > acy;
  const sy = down ? a.y + ah : a.y, ey = down ? b.y : b.y + bh;
  const left = Math.max(a.x, b.x), right = Math.min(a.x + a.w, b.x + b.w);
  let sx: number, ex: number;
  if (a.w > 300 && b.w > 300 && right - left > 40) {
    sx = ex = (left + right) / 2;
  } else {
    sx = a.w > 300 ? clamp(bcx, a.x + 16, a.x + a.w - 16) : acx;
    ex = b.w > 300 ? clamp(sx, b.x + 16, b.x + b.w - 16) : bcx;
    if (a.w > 300 && b.w <= 300) sx = clamp(ex, a.x + 16, a.x + a.w - 16);
  }
  const my = (sy + ey) / 2;
  return `M${sx},${sy} C${sx},${my} ${ex},${my} ${ex},${ey}`;
}

/** Where a component's source lives on GitHub. */
export function sourceUrl(component: Component): string {
  return `https://github.com/justmoon/dassie/tree/main/${component.path}`;
}
