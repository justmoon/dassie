import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { create } from "@bufbuild/protobuf";
import { NetworkPeerSchema, RelayConnection } from "@dassie/rpc/proto";
import { relayState, uplinkConnected, uplinkState } from "./periphery.ts";

const NOW_MS = 1_800_000_000_000;

test("an uplink says whether it is connected, and since when it is not", () => {
  const online = create(NetworkPeerSchema, { node: "da", online: true });
  assertEquals(uplinkState(online, NOW_MS), "connected");
  assertEquals(uplinkConnected(online), true);
  const silent = create(NetworkPeerSchema, {
    node: "da",
    online: false,
    lastSeenUnixMs: BigInt(NOW_MS - 120_000),
  });
  assertEquals(
    uplinkState(silent, NOW_MS),
    "not connected, last heard from 2 min ago",
  );
  assertEquals(uplinkConnected(silent), false);
  const never = create(NetworkPeerSchema, { node: "da" });
  assertEquals(uplinkState(never, NOW_MS), "not connected yet");
  assertEquals(uplinkState(undefined, NOW_MS), "not connected yet");
});

test("the relay's state reads as the node reports it", () => {
  const relay = (connection: RelayConnection, connected = false) => ({
    url: "https://relay.dassiepay.net",
    connected,
    connection,
  });
  assertEquals(relayState(relay(RelayConnection.CONNECTED, true)).tone, "good");
  assertEquals(relayState(relay(RelayConnection.CONNECTING)), {
    text: "connecting",
    tone: "plain",
  });
  assertEquals(relayState(relay(RelayConnection.UNREACHABLE)).tone, "bad");
  assertEquals(relayState(null).tone, "bad");
  // A node that reports no state of the three still says whether it is
  // connected.
  assertEquals(
    relayState(relay(RelayConnection.UNSPECIFIED, true)).text,
    "connected",
  );
  assertEquals(
    relayState(relay(RelayConnection.UNSPECIFIED)).text,
    "not connected",
  );
});
