import { test } from "node:test";
import { assertEquals, assertMatch } from "@std/assert";
import { create } from "@bufbuild/protobuf";
import {
  ErrorCode,
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
  afterWithdrawalFailed,
  automaticText,
  durationText,
  GAS_GAVE_UP_TEXT,
  GAS_RETRIES,
  GAS_RETRY_MS,
  givenUpText,
  holdRows,
  holdsTotal,
  hubConnectionText,
  hubCurrency,
  hubIssuerText,
  hubStandingText,
  isGasCheck,
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

test("a hub its token's issuer froze says so, and since when", () => {
  assertEquals(hubIssuerText({ escrowHeld: false }, NOW), undefined);
  const frozen = {
    frozenSinceUnixMs: BigInt(NOW - 120_000),
    escrowHeld: true,
    connection: LedgerConnection.SYNCING,
  };
  assertMatch(
    hubIssuerText(frozen, NOW)!,
    /holds the hub's escrow, which froze the hub 2 min ago/,
  );
  assertMatch(hubIssuerText(frozen, NOW)!, /no payment crosses this hub/);
  // Its account follows the hub all the same: not "reading".
  assertEquals(hubStandingText(frozen), undefined);
  // One the node cannot follow says so all the same.
  assertMatch(
    hubStandingText({ ...frozen, connection: LedgerConnection.FAILED })!,
    /cannot follow/,
  );
  assertMatch(
    hubStandingText({ connection: LedgerConnection.SYNCING })!,
    /Reading/,
  );
  // An ended hub the issuer still holds pays out nothing until it lets go.
  assertMatch(
    hubIssuerText({ escrowHeld: true }, NOW)!,
    /pays out nothing until the issuer lets go/,
  );
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
  // Less than a second left is no pause; a second or more never reads as
  // nothing.
  assertEquals(
    automaticText(
      { automatic: true, pausedUntilUnixMs: BigInt(NOW + 400) },
      NOW,
    ),
    "Your node keeps this account between its floor and ceiling by itself.",
  );
  assertMatch(
    automaticText(
      { automatic: true, pausedUntilUnixMs: BigInt(NOW + 1_200) },
      NOW,
    ),
    /for 2 s more/,
  );
});

test("a duration left, rounded up", () => {
  assertEquals(durationText(1_000), "1 s");
  assertEquals(durationText(58_500), "59 s");
  assertEquals(durationText(59_001), "1 min");
  assertEquals(durationText(90_000), "2 min");
  assertEquals(durationText(3_600_000), "1 h");
  assertEquals(durationText(86_400_000), "1 d");
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

/** The node's words while it checks a token hub's escrow's XRP, as
 * `hub_withdraw_from` says them. */
const CHECKING =
  "the hub's escrow pays its fees in XRP: the node checks it holds enough, and tops it up first if not; try again shortly";

test("a withdrawal is sent again while the node checks the escrow's XRP", () => {
  const checking = { code: ErrorCode.RESOURCE_EXHAUSTED, message: CHECKING };
  assertEquals(isGasCheck(checking), true);
  for (let retries = 0; retries < GAS_RETRIES; retries += 1) {
    assertEquals(afterWithdrawalFailed(checking, retries), {
      kind: "retry",
      afterMs: GAS_RETRY_MS,
    });
  }
  // A bounded number of times.
  assertEquals(afterWithdrawalFailed(checking, GAS_RETRIES), {
    kind: "stop",
    text: GAS_GAVE_UP_TEXT,
  });
});

test("any other failure of a withdrawal stops it", () => {
  const others = [
    // A top-up that failed: the node checks again only after its backoff.
    {
      code: ErrorCode.RESOURCE_EXHAUSTED,
      message:
        "the hub's escrow pays its fees in XRP, and the node's top-up of it failed: it checks again in 60 s",
    },
    {
      code: ErrorCode.RESOURCE_EXHAUSTED,
      message: "a move between the hub and the XRP Ledger is on its way",
    },
    // The same words with another code, or none.
    { code: ErrorCode.FAILED_PRECONDITION, message: CHECKING },
    { message: CHECKING },
    {
      code: ErrorCode.FAILED_PRECONDITION,
      message: "more than the hub account can spend, with the fees",
    },
  ];
  for (const failure of others) {
    assertEquals(isGasCheck(failure), false, failure.message);
    assertEquals(afterWithdrawalFailed(failure, 0), {
      kind: "stop",
      text: failure.message,
    });
  }
});
