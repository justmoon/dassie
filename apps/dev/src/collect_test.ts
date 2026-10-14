import { assertEquals } from "@std/assert";
import { create } from "@bufbuild/protobuf";
import { SourcedLogLineSchema } from "@dassie/rpc/dev";
import {
  NetworkNodeSchema,
  NodeTableEventSchema,
  PacketSchema,
  PacketState,
  PeerState,
} from "@dassie/rpc/proto";
import { LogRing, PeerSet, shortName, TrafficFilter } from "./collect.ts";

Deno.test("the log ring keeps the last lines", () => {
  const ring = new LogRing(3);
  const line = (n: number) =>
    create(SourcedLogLineSchema, {
      source: "d1",
      line: { index: BigInt(n), message: `${n}` },
    });
  ring.add([line(1), line(2)]);
  ring.add([line(3), line(4), line(5)]);
  assertEquals(ring.lines.map((l) => l.line!.message), ["3", "4", "5"]);
});

Deno.test("a node's peers follow its node table", () => {
  const peers = new PeerSet();
  const node = (id: string, peerState: PeerState) =>
    create(NetworkNodeSchema, { id, peerState });
  const event = (
    init: {
      reset?: boolean;
      upserted?: ReturnType<typeof node>[];
      removed?: string[];
    },
  ) => create(NodeTableEventSchema, init);
  assertEquals(
    peers.apply(event({
      reset: true,
      upserted: [node("b", PeerState.PEERED), node("a", PeerState.REQUESTING)],
    })),
    true,
  );
  assertEquals(peers.peers(), ["b"]);
  assertEquals(
    peers.apply(event({ upserted: [node("c", PeerState.NONE)] })),
    false,
  );
  assertEquals(
    peers.apply(event({ upserted: [node("a", PeerState.PEERED)] })),
    true,
  );
  assertEquals(peers.peers(), ["a", "b"]);
  assertEquals(peers.apply(event({ removed: ["b"] })), true);
  assertEquals(peers.apply(event({ reset: true })), true);
  assertEquals(peers.peers(), []);
});

Deno.test("each packet to a peer counts once", () => {
  const filter = new TrafficFilter();
  const packet = (sequence: bigint, destination: string, state: PacketState) =>
    create(PacketSchema, {
      trace: { epoch: 1n, sequence },
      destination,
      state,
    });
  assertEquals(
    filter.hop(packet(1n, "peer:d2_x", PacketState.PENDING)),
    "d2_x",
  );
  assertEquals(
    filter.hop(packet(1n, "peer:d2_x", PacketState.FULFILLED)),
    undefined,
  );
  assertEquals(
    filter.hop(packet(2n, "local:0", PacketState.PENDING)),
    undefined,
  );
  assertEquals(
    filter.hop(packet(3n, "peer:d3_y", PacketState.REJECTED)),
    "d3_y",
  );
});

Deno.test("vanity IDs have short names", () => {
  assertEquals(shortName("d12_abcdefghijklmnopqrstuvwxyz"), "d12");
  assertEquals(shortName("someone_else"), "someone_else");
});
