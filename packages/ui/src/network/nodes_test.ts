import { assertEquals } from "@std/assert";
import { create } from "@bufbuild/protobuf";
import {
  type NetworkNode,
  NetworkNodeSchema,
  PeerState,
} from "@dassie/rpc/proto";
import { graphLinks, groupNodes, seedIndex, sortNodes } from "./nodes.ts";

function node(
  id: string,
  distance: number | undefined,
  neighbors: string[] = [],
  peerState: PeerState = PeerState.NONE,
): NetworkNode {
  return create(NetworkNodeSchema, { id, distance, neighbors, peerState });
}

Deno.test("nodes sort by distance, then ID, and group under headings", () => {
  const nodes = [
    node("dz", undefined),
    node("dc", 2),
    node("db", 1, [], PeerState.PEERED),
    node("da", 1, [], PeerState.PEERED),
    node("dself", 0),
    node("dp", undefined, [], PeerState.TERMINATING),
  ];
  assertEquals(sortNodes(nodes).map((n) => n.id), [
    "dself",
    "da",
    "db",
    "dp",
    "dc",
    "dz",
  ]);
  assertEquals(groupNodes(nodes).map((g) => [g.heading, g.nodes.length]), [
    ["This Node", 1],
    ["Direct Peers", 2],
    ["Peers Without a Route", 1],
    ["Other Nodes", 1],
    ["No Connection", 1],
  ]);
});

Deno.test("links: ours to peers, others' only when both list each other", () => {
  const nodes = [
    node("da", 0),
    node("db", 1, ["da", "dc"], PeerState.PEERED),
    node("dc", 2, ["db"]),
    node("dd", 2, ["db"]),
  ];
  assertEquals(graphLinks(nodes, "da"), [
    { a: "da", b: "db" },
    { a: "db", b: "dc" },
  ]);
});

Deno.test("colors are stable per ID", () => {
  assertEquals(seedIndex("d1"), seedIndex("d1"));
  const spread = new Set(
    ["d1", "d2", "d3", "d4", "d5", "d6"].map((id) => seedIndex(id)),
  );
  assertEquals(spread.size > 1, true);
});
