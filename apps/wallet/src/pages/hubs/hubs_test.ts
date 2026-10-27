import { test } from "node:test";
import { assertEquals, assertMatch } from "@std/assert";
import { create } from "@bufbuild/protobuf";
import {
  GetHubPayoutResponseSchema,
  HubHoldSchema,
  HubMove_Direction as Direction,
  HubMoveSchema,
  HubPayoutBundleSchema,
  HubStatusSchema,
  LedgerConnection,
} from "@dassie/rpc/proto";
import { currencyOf } from "@dassie/ui/currency";
import {
  automaticText,
  givenUpText,
  holdRows,
  holdsTotal,
  hubConnectionText,
  hubCurrency,
  moveText,
  payoutState,
  payoutText,
  validatorsText,
  withHub,
} from "./hubs.ts";

const XRP = currencyOf("XRP", 9);
const NOW = 1_800_000_000_000;

test("a hub's amounts are in its scheme's ledger's asset", () => {
  const ledgers = [
    { settlementScheme: "xrpl-testnet-xrp", assetCode: "XRP", assetScale: 9 },
    {
      settlementScheme: "hub-xrpl-testnet-dsusd",
      assetCode: "USD",
      assetScale: 9,
    },
  ];
  assertEquals(
    hubCurrency("hub-xrpl-testnet-dsusd", ledgers),
    currencyOf("USD", 9),
  );
  // Never assumed: no ledger of the scheme, no currency.
  assertEquals(hubCurrency("hub-xrpl-testnet-xrp", ledgers), undefined);
});

test("the account's connection is said only when it is not up to date", () => {
  assertEquals(hubConnectionText(LedgerConnection.SYNCED), undefined);
  assertMatch(
    hubConnectionText(LedgerConnection.UNFUNDED)!,
    /first deposit opens it/,
  );
  assertMatch(hubConnectionText(LedgerConnection.FAILED)!, /cannot follow/);
});

test("validators answering", () => {
  assertEquals(validatorsText(4, 3), "3 of 4 validators answering");
  assertEquals(validatorsText(1, 0), "0 of 1 validator answering");
});

test("the move on its way says which way, how much, who and since when", () => {
  const deposit = create(HubMoveSchema, {
    direction: Direction.DEPOSIT,
    amount: 2_500_000_000n,
    byOwner: true,
    sentAtUnixMs: BigInt(NOW - 120_000),
  });
  assertEquals(
    moveText(deposit, XRP, NOW),
    "Depositing XRP 2.50 into the hub: you asked, sent 2 min ago.",
  );
  const withdrawal = create(HubMoveSchema, {
    direction: Direction.WITHDRAW,
    amount: 1_000_000_000n,
    sentAtUnixMs: BigInt(NOW - 600_000),
    appliedAtUnixMs: BigInt(NOW - 60_000),
  });
  assertEquals(
    moveText(withdrawal, XRP, NOW),
    "Withdrawing XRP 1.00 out of the hub: your node moves it by itself, the XRP Ledger applied it 1 min ago.",
  );
  assertMatch(givenUpText(deposit, XRP), /^You gave up a deposit of XRP 2\.50/);
});

test("whether the node moves funds by itself, and its pause", () => {
  assertMatch(
    automaticText({ automatic: false }, NOW),
    /moves nothing by itself/,
  );
  assertEquals(
    automaticText({ automatic: true }, NOW),
    "Your node keeps this account between its floor and ceiling by itself.",
  );
  // A pause that is over says nothing of it.
  assertEquals(
    automaticText(
      { automatic: true, pausedUntilUnixMs: BigInt(NOW - 1) },
      NOW,
    ),
    "Your node keeps this account between its floor and ceiling by itself.",
  );
  assertEquals(
    automaticText(
      { automatic: true, pausedUntilUnixMs: BigInt(NOW + 3_600_000) },
      NOW,
    ),
    "Your last move pauses your node's own moves for 1 h more. Then it keeps this account between its floor and ceiling by itself again.",
  );
});

test("holds are listed as the node reports them", () => {
  const holds = [
    create(HubHoldSchema, { peer: "dA", account: 7, amount: 5n }),
    create(HubHoldSchema, { account: 9, amount: 3n }),
  ];
  assertEquals(holdRows(holds), [
    { peer: "dA", account: 7, amount: 5n },
    { peer: undefined, account: 9, amount: 3n },
  ]);
  assertEquals(holdsTotal(holds), 8n);
  assertEquals(holdsTotal([]), 0n);
});

test("the payout: nothing while the hub runs, then signing, then ready", () => {
  const running = create(GetHubPayoutResponseSchema, { quorum: 3 });
  assertEquals(payoutState(running), { kind: "running" });
  assertEquals(payoutText(payoutState(running)), undefined);
  const bundle = (validator: number) =>
    create(HubPayoutBundleSchema, { validator, height: 40n, transactions: 2 });
  const signing = create(GetHubPayoutResponseSchema, {
    quorum: 3,
    bundles: [bundle(0), bundle(2)],
  });
  assertEquals(payoutState(signing), { kind: "signing", signed: 2, quorum: 3 });
  assertMatch(payoutText(payoutState(signing))!, /2 of the 3 it takes/);
  const ready = create(GetHubPayoutResponseSchema, {
    quorum: 3,
    bundles: [bundle(0), bundle(1), bundle(2)],
    readyHeight: 40n,
    transactions: ["12ab", "12cd"],
  });
  assertEquals(payoutState(ready), {
    kind: "ready",
    height: 40n,
    transactions: ["12ab", "12cd"],
  });
  assertMatch(payoutText(payoutState(ready))!, /of block 40: 2 transactions/);
});

test("an action's answer replaces its hub only", () => {
  const a = create(HubStatusSchema, { hubId: "aa", balance: 1n });
  const b = create(HubStatusSchema, { hubId: "bb", balance: 2n });
  const changed = create(HubStatusSchema, { hubId: "bb", balance: 5n });
  assertEquals(
    withHub([a, b], changed).map((hub) => [hub.hubId, hub.balance]),
    [["aa", 1n], ["bb", 5n]],
  );
});
