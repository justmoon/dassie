import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { atFrontDoor, frontDoorState } from "./front_door.ts";

const label = "gstpaogawdidju5a6m4lolevaes3cm4dj7omffq";

test("pointers at the default front door are told apart", () => {
  assertEquals(atFrontDoor(`$${label}.dassiepay.net`), true);
  assertEquals(atFrontDoor(`$${label}.DassiePay.net/pay/xrpl`), true);
  assertEquals(atFrontDoor("$node.example"), false);
  assertEquals(atFrontDoor("$node.example/pay/stub"), false);
  assertEquals(atFrontDoor("$dassiepay.net.example"), false);
});

test("a name at the front door is reachable while the node is connected to it", () => {
  const pointer = `$${label}.dassiepay.net`;
  const door = "https://relay.dassiepay.net";
  assertEquals(
    frontDoorState(pointer, { url: door, connected: true }),
    "reachable",
  );
  assertEquals(
    frontDoorState(pointer, { url: door, connected: false }),
    "disconnected",
  );
  // Opted out with an empty DASSIE_RELAY.
  assertEquals(frontDoorState(pointer, null), "no-relay");
  // Another relay does not take the front door's requests.
  const other = { url: "https://relay.example", connected: true };
  assertEquals(frontDoorState(pointer, other), "other-relay");
  const lookalike = { url: "https://dassiepay.net.example", connected: true };
  assertEquals(frontDoorState(pointer, lookalike), "other-relay");
  assertEquals(frontDoorState(pointer, undefined), "unknown");
});

test("nothing is said of a name elsewhere", () => {
  const relay = { url: "https://relay.dassiepay.net", connected: false };
  assertEquals(frontDoorState("$node.example", relay), undefined);
  assertEquals(frontDoorState("$node.example", null), undefined);
  assertEquals(frontDoorState(undefined, relay), undefined);
});
