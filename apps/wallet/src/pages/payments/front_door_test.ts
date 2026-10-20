import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { atFrontDoor } from "./front_door.ts";

test("pointers at the default front door are told apart", () => {
  const label = "gstpaogawdidju5a6m4lolevaes3cm4dj7omffq";
  assertEquals(atFrontDoor(`$${label}.dassiepay.net`), true);
  assertEquals(atFrontDoor(`$${label}.DassiePay.net/pay/xrpl`), true);
  assertEquals(atFrontDoor("$node.example"), false);
  assertEquals(atFrontDoor("$node.example/pay/stub"), false);
  assertEquals(atFrontDoor("$dassiepay.net.example"), false);
});
