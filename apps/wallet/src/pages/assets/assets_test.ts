import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { create } from "@bufbuild/protobuf";
import {
  ExternalAssetSchema,
  HoldingSchema,
  LedgerConnection,
} from "@dassie/rpc/proto";
import {
  assetName,
  assetOf,
  freezeText,
  holdingCurrency,
  holdingOf,
  nativeAsset,
} from "./assets.ts";

const xrp = create(HoldingSchema, {
  assetCode: "XRP",
  amount: 25_000_000n,
  assetScale: 6,
  reserve: 1_200_000n,
});
const rlusd = create(HoldingSchema, {
  assetCode: "RLUSD",
  issuer: "rMxCKbEDwqr76QuheSUMdEGf4B9xJ8m5De",
  amount: 12_500_000_000_000_000n,
  assetScale: 15,
});
const account = create(ExternalAssetSchema, {
  id: "xrpl-account/0",
  address: "rAccount",
  holdings: [xrp, rlusd],
  connection: LedgerConnection.SYNCED,
  settlementSchemes: ["xrpl-xrp", "xrpl-rlusd"],
  faucet: false,
});

test("an asset is named by its kind, and numbered past the first", () => {
  assertEquals(
    assetName("xrpl-testnet-account/0"),
    "XRP Ledger Testnet account",
  );
  assertEquals(
    assetName("xrpl-testnet-account/1"),
    "XRP Ledger Testnet account 2",
  );
  assertEquals(assetName("xrpl-account/0"), "XRP Ledger account");
  assertEquals(assetName("bank/0"), "bank");
  assertEquals(nativeAsset("xrpl-testnet-account/0"), "XRP");
  assertEquals(nativeAsset("xrpl-account/0"), "XRP");
  assertEquals(nativeAsset("bank/0"), undefined);
  // A hub account is named by the scheme over its hub, with no number.
  assertEquals(assetName("hub-xrpl-testnet-dsusd"), "hub-xrpl-testnet-dsusd");
  assertEquals(nativeAsset("hub-xrpl-testnet-dsusd"), undefined);
});

test("a holding is shown in its own asset, at its own scale", () => {
  const currency = holdingCurrency(rlusd);
  assertEquals([currency.code, currency.scale], ["RLUSD", 15]);
  assertEquals(holdingCurrency(xrp).symbol, "XRP");
});

test("a freeze is told, the deepest first, and nothing without one", () => {
  const none = { frozen: false, deepFrozen: false, globalFreeze: false };
  assertEquals(freezeText(none), undefined);
  assert(freezeText({ ...none, frozen: true })?.includes("still receives"));
  assert(
    freezeText({ ...none, frozen: true, deepFrozen: true })?.includes(
      "neither pays nor receives",
    ),
  );
  assert(
    freezeText({ ...none, frozen: true, globalFreeze: true })?.includes(
      "every holder",
    ),
  );
});

test("a scheme's asset and its holdings are found by the node's report", () => {
  assertEquals(assetOf("xrpl-rlusd", [account])?.id, account.id);
  assertEquals(assetOf("stub", [account]), undefined);
  assertEquals(holdingOf(account, "XRP")?.reserve, 1_200_000n);
  assertEquals(holdingOf(account, "RLUSD")?.issuer, rlusd.issuer);
  assertEquals(holdingOf(undefined, "XRP"), undefined);
});
