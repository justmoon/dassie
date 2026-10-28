import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { create } from "@bufbuild/protobuf";
import {
  type NetworkNode,
  NetworkNodeSchema,
  PeerState,
} from "@dassie/rpc/proto";
import {
  groupNodes,
  routingGraph,
  seedIndex,
  sortNodes,
  uplinks,
} from "./nodes.ts";

function node(
  id: string,
  distance: number | undefined,
  neighbors: string[] = [],
  peerState: PeerState = PeerState.NONE,
  online = true,
): NetworkNode {
  const peered = peerState === PeerState.PEERED ||
    peerState === PeerState.TERMINATING;
  return create(NetworkNodeSchema, {
    id,
    isSelf: distance === 0,
    distance,
    neighbors,
    peerState,
    peering: peered ? { node: id, online } : undefined,
  });
}

test("nodes sort by distance, then ID, and group under headings", () => {
  const nodes = [
    node("dz", undefined),
    node("dc", 2),
    node("db", 1, [], PeerState.PEERED),
    node("da", 1, [], PeerState.PEERED),
    node("dself", 0),
    node("dp", undefined, [], PeerState.TERMINATING),
    node("do", undefined, [], PeerState.PEERED, false),
  ];
  assertEquals(sortNodes(nodes).map((n) => n.id), [
    "dself",
    "da",
    "db",
    "do",
    "dp",
    "dc",
    "dz",
  ]);
  assertEquals(
    groupNodes(nodes).map((g) => [g.heading, g.nodes.map((n) => n.id)]),
    [
      ["This node", ["dself"]],
      ["Direct peers", ["da", "db"]],
      ["Peers not routed", ["dp"]],
      ["Offline peers", ["do"]],
      ["Other nodes", ["dc"]],
      ["No connection", ["dz"]],
    ],
  );
});

test("the graph is the routing table: reached nodes, edges both list", () => {
  const nodes = [
    node("da", 0, ["db", "dc"]),
    node("db", 1, ["da", "dc", "dz"], PeerState.PEERED),
    node("dc", 2, ["db"]),
    node("dd", 2, ["db"]),
    // A peer we still list whose new link state has not dropped us yet...
    node("de", undefined, ["da"], PeerState.PEERED),
    // ...a peer gone offline, still listing us in its last link state...
    node("do", undefined, ["da"], PeerState.PEERED, false),
    // ...and two nodes no route reaches, listing each other.
    node("dy", undefined, ["dz"]),
    node("dz", undefined, ["db", "dy"]),
  ];
  const graph = routingGraph(nodes);
  assertEquals(graph.nodes.map((n) => n.id), ["da", "db", "dc", "dd"]);
  assertEquals(graph.links, [
    { a: "da", b: "db" },
    { a: "db", b: "dc" },
  ]);
});

test("the graph keeps this node when nothing is reached", () => {
  const graph = routingGraph([
    node("da", 0),
    node("do", undefined, ["da"], PeerState.PEERED, false),
  ]);
  assertEquals(graph.nodes.map((n) => n.id), ["da"]);
  assertEquals(graph.links, []);
});

// A periphery node's uplinks do not list it, so only its own peerings say
// where its packets go.
const periphery = [
  node("dm", 0),
  node("db", 1, ["dc", "dd", "dl"], PeerState.PEERED),
  node("dc", 1, ["db", "dd"], PeerState.PEERED),
  node("dd", 2, ["db", "dc"]),
  // An uplink gone offline: no route reaches it...
  node("do", undefined, [], PeerState.PEERED, false),
  // ...and one gone offline that another uplink still reaches: no first
  // hop, so no line from this node.
  node("dl", 2, ["db"], PeerState.PEERED, false),
  // A peering still being asked for is no uplink yet.
  node("dr", undefined, [], PeerState.REQUESTING),
];

test("a periphery node is drawn with a line to each uplink a route reaches", () => {
  const graph = routingGraph(periphery, true);
  assertEquals(graph.nodes.map((n) => n.id), ["dm", "db", "dc", "dd", "dl"]);
  assertEquals(graph.links, [
    { a: "db", b: "dc" },
    { a: "db", b: "dd" },
    { a: "db", b: "dl" },
    { a: "db", b: "dm" },
    { a: "dc", b: "dd" },
    { a: "dc", b: "dm" },
  ]);
  // A core node's own link state draws its lines; nothing is added.
  assertEquals(
    routingGraph(periphery).links.some((l) => l.b === "dm"),
    false,
  );
});

test("a periphery node's uplinks are its peers, online or not", () => {
  // In the listing's order: a peer no route reaches sorts just after the
  // first hops.
  assertEquals(uplinks(periphery).map((n) => n.id), [
    "db",
    "dc",
    "do",
    "dl",
  ]);
});

test("colors are stable per ID", () => {
  assertEquals(seedIndex("d1"), seedIndex("d1"));
  const spread = new Set(
    ["d1", "d2", "d3", "d4", "d5", "d6"].map((id) => seedIndex(id)),
  );
  assertEquals(spread.size > 1, true);
});
