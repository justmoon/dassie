import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { create } from "@bufbuild/protobuf";
import { NetworkPeerSchema, PeerMethodSchema } from "@dassie/rpc/proto";
import { methodState, peerMethods } from "./methods.ts";

test("every method of a peering is listed, in its order", () => {
  const peer = create(NetworkPeerSchema, {
    node: "da",
    methods: [
      create(PeerMethodSchema, { scheme: "hub-xrpl-testnet-xrp", owed: 5n }),
      create(PeerMethodSchema, {
        scheme: "hub-xrpl-testnet-dsusd",
        owed: -7n,
        windingDown: true,
      }),
    ],
  });
  assertEquals(
    peerMethods(peer).map((method) => [method.scheme, method.owed]),
    [
      ["hub-xrpl-testnet-xrp", 5n],
      ["hub-xrpl-testnet-dsusd", -7n],
    ],
  );
});

test("a method shows when it winds down or is short of room", () => {
  const method = {
    scheme: "stub",
    owed: 0n,
    withdrawn: false,
    windingDown: false,
  };
  assertEquals(methodState(method), undefined);
  assertEquals(
    methodState({ ...method, withdrawn: true })?.label,
    "Short of room",
  );
  assertEquals(
    methodState({ ...method, withdrawn: true, windingDown: true })?.label,
    "Winding down",
  );
});
