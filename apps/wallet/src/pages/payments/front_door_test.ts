import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { RelayConnection } from "@dassie/rpc/proto";
import { atFrontDoor, frontDoorState, type Relay } from "./front_door.ts";

const label = "gstpaogawdidju5a6m4lolevaes3cm4dj7omffq";

/** A relay at `url` as the node reports it, in `connection`. */
function relay(url: string, connection: RelayConnection): Relay {
  return {
    url,
    connected: connection === RelayConnection.CONNECTED,
    connection,
  };
}

test("pointers at the default front door are told apart", () => {
  assertEquals(atFrontDoor(`$${label}.dassiepay.net`), true);
  assertEquals(atFrontDoor(`$${label}.DassiePay.net/pay/xrp`), true);
  assertEquals(atFrontDoor("$node.example"), false);
  assertEquals(atFrontDoor("$node.example/pay/usd"), false);
  assertEquals(atFrontDoor("$dassiepay.net.example"), false);
});

test("a name at the front door is reachable while the node is connected to it", () => {
  const pointer = `$${label}.dassiepay.net`;
  const door = "https://relay.dassiepay.net";
  const { CONNECTED, CONNECTING, UNREACHABLE } = RelayConnection;
  assertEquals(frontDoorState(pointer, relay(door, CONNECTED)), "reachable");
  // Right after a start or a brief drop: no fault yet.
  assertEquals(frontDoorState(pointer, relay(door, CONNECTING)), "connecting");
  assertEquals(
    frontDoorState(pointer, relay(door, UNREACHABLE)),
    "disconnected",
  );
  // Without a state (UNSPECIFIED), or with one the wallet does not know,
  // whether it is connected decides.
  const { UNSPECIFIED } = RelayConnection;
  const unsaid = { url: door, connected: false, connection: UNSPECIFIED };
  assertEquals(frontDoorState(pointer, unsaid), "disconnected");
  const held = { ...unsaid, connected: true };
  assertEquals(frontDoorState(pointer, held), "reachable");
  // Opted out with an empty DASSIE_RELAY.
  assertEquals(frontDoorState(pointer, null), "no-relay");
  // Another relay does not take the front door's requests.
  const other = relay("https://relay.example", CONNECTED);
  assertEquals(frontDoorState(pointer, other), "other-relay");
  const lookalike = relay("https://dassiepay.net.example", CONNECTED);
  assertEquals(frontDoorState(pointer, lookalike), "other-relay");
  assertEquals(frontDoorState(pointer, undefined), "unknown");
});

test("nothing is said of a name elsewhere", () => {
  const door = relay(
    "https://relay.dassiepay.net",
    RelayConnection.UNREACHABLE,
  );
  assertEquals(frontDoorState("$node.example", door), undefined);
  assertEquals(frontDoorState("$node.example", null), undefined);
  assertEquals(frontDoorState(undefined, door), undefined);
});
