import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { assetTab } from "./receive.ts";

// spec/areas/wallet.md#pages: a link to an owner book's tab lands on its
// asset's, the hub's while funds are kept there, and the other way round.
test("a link names its asset's tab", () => {
  const inHub = ["open-payments", "stub", "hub-xrpl-testnet-xrp"];
  assertEquals(assetTab("xrpl-testnet-xrp", inHub), "hub-xrpl-testnet-xrp");
  assertEquals(assetTab("stub", inHub), "stub");
  const own = ["open-payments", "xrpl-testnet-xrp"];
  assertEquals(assetTab("hub-xrpl-testnet-xrp", own), "xrpl-testnet-xrp");
  assertEquals(assetTab("xrpl-testnet-xrp", own), "xrpl-testnet-xrp");
  // A scheme of no asset's tab stays as it is: the first tab shows.
  assertEquals(assetTab("xrpl-testnet-dsusd", own), "xrpl-testnet-dsusd");
});
