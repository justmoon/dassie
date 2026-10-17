import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { create } from "@bufbuild/protobuf";
import {
  type NetworkNode,
  NetworkNodeSchema,
  PeerState,
} from "@dassie/rpc/proto";
import { groupNodes, routingGraph, seedIndex, sortNodes } from "./nodes.ts";

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
      ["This Node", ["dself"]],
      ["Direct Peers", ["da", "db"]],
      ["Peers Not Routed", ["dp"]],
      ["Offline Peers", ["do"]],
      ["Other Nodes", ["dc"]],
      ["No Connection", ["dz"]],
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

test("colors are stable per ID", () => {
  assertEquals(seedIndex("d1"), seedIndex("d1"));
  const spread = new Set(
    ["d1", "d2", "d3", "d4", "d5", "d6"].map((id) => seedIndex(id)),
  );
  assertEquals(spread.size > 1, true);
});
