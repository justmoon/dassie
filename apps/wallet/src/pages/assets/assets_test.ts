import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { create } from "@bufbuild/protobuf";
import {
  AssetBalanceSchema,
  ExternalAssetSchema,
  HoldingSchema,
  LedgerConnection,
  Realm,
} from "@dassie/rpc/proto";
import {
  assetLedgers,
  assetName,
  assetOf,
  assetTitle,
  breakdownRows,
  freezeText,
  holdingCurrency,
  holdingOf,
  nativeAsset,
  showsSpendable,
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

// XRP on the testnet, in millionths: 15.20 in the account, 1.20 of it the
// reserve; in the hub 3.80, of it 0.75 held for peers and 2.00 the hub's
// reserves; 0.50 on its way into the hub.
const xrpBalance = create(AssetBalanceSchema, {
  name: "xrp",
  assetCode: "XRP",
  assetScale: 6,
  realm: Realm.TEST,
  ledgers: ["hub-xrpl-testnet-xrp+xrp", "xrpl-testnet-xrp+xrp"],
  paysFrom: "hub-xrpl-testnet-xrp+xrp",
  ownerBalance: 19_500_000n,
  spendable: 15_550_000n,
  account: 15_200_000n,
  accountReserve: 1_200_000n,
  hub: 3_800_000n,
  hubReserve: 2_000_000n,
  heldForPeers: 750_000n,
  onItsWay: 500_000n,
});

test("an asset is named for people by its name and realm", () => {
  assertEquals(assetTitle(xrpBalance), "Test XRP");
  assertEquals(assetTitle({ name: "xrp", realm: Realm.LIVE }), "XRP");
  assertEquals(assetTitle({ name: "eur", realm: Realm.TEST }), "EUR");
  assertEquals(assetLedgers(xrpBalance), {
    book: "xrpl-testnet-xrp+xrp",
    hub: "hub-xrpl-testnet-xrp+xrp",
  });
});

test("a balance is broken down by place, with what each sets aside", () => {
  assertEquals(breakdownRows(xrpBalance), [
    {
      id: "account",
      label: "In your account",
      amount: 15_200_000n,
      sub: [{ amount: 1_200_000n }, " of it is the XRP Ledger's reserve."],
    },
    {
      id: "hub",
      label: "In the hub",
      amount: 3_800_000n,
      sub: [
        { amount: 1_050_000n },
        " to spend, ",
        { amount: 750_000n },
        " held for your peers, ",
        { amount: 2_000_000n },
        " the hub's reserves.",
      ],
    },
    {
      id: "on-its-way",
      label: "On its way",
      amount: 500_000n,
      sub: ["Moving between your account and the hub."],
    },
  ]);
  assertEquals(showsSpendable(xrpBalance), true);
});

test("a part with nothing set aside says nothing of it", () => {
  const quiet = create(AssetBalanceSchema, {
    ...xrpBalance,
    accountReserve: 0n,
    heldForPeers: 0n,
    hubReserve: 2_000_000n,
    hub: 1_000_000n,
    onItsWay: 0n,
  });
  const rows = breakdownRows(quiet);
  assertEquals(rows.map((row) => row.id), ["account", "hub"]);
  assertEquals(rows[0].sub, []);
  // Less in the hub than it sets aside leaves nothing to spend there.
  assertEquals(rows[1].sub, [
    { amount: 2_000_000n },
    " the hub's reserves.",
  ]);
});

test("a token's reserve is in XRP, and the stub has no places", () => {
  const dsusd = create(AssetBalanceSchema, {
    name: "dsusd",
    realm: Realm.TEST,
    ledgers: ["xrpl-testnet-dsusd+usd"],
    account: 42_500_000n,
  });
  assertEquals(breakdownRows(dsusd), [{
    id: "account",
    label: "In your account",
    amount: 42_500_000n,
    sub: ["Its reserve is in XRP, counted with your XRP."],
  }]);
  const stub = create(AssetBalanceSchema, {
    name: "usd",
    ledgers: ["stub+usd"],
    ownerBalance: 100n,
    spendable: 100n,
  });
  assertEquals(breakdownRows(stub), []);
  assertEquals(showsSpendable(stub), false);
  assertEquals(showsSpendable({ ownerBalance: 0n, spendable: -5n }), false);
});
