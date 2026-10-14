/**
 * The node table as the network page lists it (spec 6 §1.6.10): sorted by
 * distance, then by ID, and grouped under the original's headings; and the
 * graph's links.
 */

import { type NetworkNode, PeerState } from "@dassie/rpc/proto";

/** A group of the listing. */
export interface NodeGroup {
  /** For the column: "0", "1", "2", "∞", or "–" for unrouted peers. */
  mark: string;
  heading: string;
  nodes: NetworkNode[];
}

const isPeered = (node: NetworkNode) =>
  node.peerState === PeerState.PEERED ||
  node.peerState === PeerState.TERMINATING;

/** Where a node sorts: its distance, peers without a route just after 1. */
function rank(node: NetworkNode): number {
  if (node.distance !== undefined) return node.distance;
  return isPeered(node) ? 1.5 : Number.POSITIVE_INFINITY;
}

/** The nodes by distance, then ID. */
export function sortNodes(nodes: readonly NetworkNode[]): NetworkNode[] {
  return [...nodes].sort((a, b) =>
    rank(a) - rank(b) || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0)
  );
}

/** The nodes in the listing's groups, in order; empty groups left out. */
export function groupNodes(nodes: readonly NetworkNode[]): NodeGroup[] {
  const groups: NodeGroup[] = [
    { mark: "0", heading: "This Node", nodes: [] },
    { mark: "1", heading: "Direct Peers", nodes: [] },
    { mark: "–", heading: "Peers Without a Route", nodes: [] },
    { mark: "2+", heading: "Other Nodes", nodes: [] },
    { mark: "∞", heading: "No Connection", nodes: [] },
  ];
  for (const node of sortNodes(nodes)) {
    const r = rank(node);
    const index = r === 0
      ? 0
      : r === 1
      ? 1
      : r === 1.5
      ? 2
      : r < Infinity
      ? 3
      : 4;
    groups[index].nodes.push(node);
  }
  return groups.filter((group) => group.nodes.length > 0);
}

/** A link of the graph, between two node IDs, `a < b`. */
export interface Link {
  a: string;
  b: string;
}

/**
 * The graph's links: ours to each peer, and between other nodes that list
 * each other as neighbors. Each link once.
 */
export function graphLinks(
  nodes: readonly NetworkNode[],
  self: string,
): Link[] {
  const byId = new Map(nodes.map((node) => [node.id, node]));
  const links = new Map<string, Link>();
  const add = (x: string, y: string) => {
    if (x === y || !byId.has(x) || !byId.has(y)) return;
    const [a, b] = x < y ? [x, y] : [y, x];
    links.set(`${a} ${b}`, { a, b });
  };
  for (const node of nodes) {
    if (node.id === self) continue;
    if (isPeered(node)) add(self, node.id);
    for (const neighbor of node.neighbors) {
      if (neighbor === self) continue;
      if (byId.get(neighbor)?.neighbors.includes(node.id)) {
        add(node.id, neighbor);
      }
    }
  }
  return [...links.values()].sort((x, y) =>
    x.a === y.a ? (x.b < y.b ? -1 : 1) : x.a < y.a ? -1 : 1
  );
}

/** A peering's state, in a word. */
export function peerStateName(state: PeerState): string {
  return ["unknown", "none", "requesting", "peered", "terminating"][state] ??
    "unknown";
}

/** A stable palette index for an ID, so a node keeps its color. */
export function seedIndex(id: string, size = 12): number {
  let hash = 0x811c9dc5;
  for (let index = 0; index < id.length; index++) {
    hash ^= id.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash % size;
}
