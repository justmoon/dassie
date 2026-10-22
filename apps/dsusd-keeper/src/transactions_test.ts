import { test } from "node:test";
import { assertEquals, assertThrows } from "@std/assert";
import {
  TF_SINGLE_ASSET,
  TF_TWO_ASSET,
  TF_TWO_ASSET_IF_EMPTY,
  TRADING_FEE_UNITS,
  transactionOf,
} from "./transactions.ts";
import { DSUSD_CURRENCY } from "./units.ts";

const ADDRESSES = { issuer: "rIssuer", keeper: "rKeeper" };
const dsusd = (value: string) => ({
  currency: DSUSD_CURRENCY,
  issuer: "rIssuer",
  value,
});
const PAIR = {
  Asset: { currency: "XRP" },
  Asset2: { currency: DSUSD_CURRENCY, issuer: "rIssuer" },
};

test("the issuer sets DefaultRipple and mints; the keeper sets its line", () => {
  assertEquals(transactionOf({ kind: "default-ripple" }, ADDRESSES), {
    signer: "issuer",
    tx: { TransactionType: "AccountSet", Account: "rIssuer", SetFlag: 8 },
  });
  assertEquals(transactionOf({ kind: "trust-line" }, ADDRESSES), {
    signer: "keeper",
    tx: {
      TransactionType: "TrustSet",
      Account: "rKeeper",
      LimitAmount: dsusd("9999999999999999e80"),
      Flags: 0x0002_0000,
    },
  });
  assertEquals(
    transactionOf({ kind: "mint", micros: 12_500_000n }, ADDRESSES),
    {
      signer: "issuer",
      tx: {
        TransactionType: "Payment",
        Account: "rIssuer",
        Destination: "rKeeper",
        Amount: dsusd("12.5"),
      },
    },
  );
  assertEquals(transactionOf({ kind: "burn", micros: 1n }, ADDRESSES), {
    signer: "keeper",
    tx: {
      TransactionType: "Payment",
      Account: "rKeeper",
      Destination: "rIssuer",
      Amount: dsusd("0.000001"),
    },
  });
});

test("the keeper makes, moves and deepens the pool", () => {
  assertEquals(
    transactionOf({
      kind: "create-pool",
      drops: 75_000_000n,
      micros: 187_500_000n,
    }, ADDRESSES),
    {
      signer: "keeper",
      tx: {
        TransactionType: "AMMCreate",
        Account: "rKeeper",
        Amount: "75000000",
        Amount2: dsusd("187.5"),
        TradingFee: TRADING_FEE_UNITS,
      },
    },
  );
  assertEquals(
    transactionOf({ kind: "refill-pool", drops: 1n, micros: 2n }, ADDRESSES)
      ?.tx,
    {
      TransactionType: "AMMDeposit",
      Account: "rKeeper",
      ...PAIR,
      Amount: "1",
      Amount2: dsusd("0.000002"),
      Flags: TF_TWO_ASSET_IF_EMPTY,
    },
  );
  assertEquals(
    transactionOf({ kind: "raise", micros: 3_000_000n }, ADDRESSES)?.tx,
    {
      TransactionType: "AMMDeposit",
      Account: "rKeeper",
      ...PAIR,
      Amount: dsusd("3"),
      Flags: TF_SINGLE_ASSET,
    },
  );
  assertEquals(
    transactionOf({ kind: "lower", micros: 3_000_000n }, ADDRESSES)?.tx,
    {
      TransactionType: "AMMWithdraw",
      Account: "rKeeper",
      ...PAIR,
      Amount: dsusd("3"),
      Flags: TF_SINGLE_ASSET,
    },
  );
  assertEquals(
    transactionOf(
      { kind: "deepen", drops: 100_000_000n, micros: 250_000_000n },
      ADDRESSES,
    )?.tx,
    {
      TransactionType: "AMMDeposit",
      Account: "rKeeper",
      ...PAIR,
      Amount: "100000000",
      Amount2: dsusd("250"),
      Flags: TF_TWO_ASSET,
    },
  );
});

test("the faucet and the ends submit nothing; no amount is out of range", () => {
  assertEquals(
    transactionOf({ kind: "fund", account: "keeper" }, ADDRESSES),
    null,
  );
  assertEquals(transactionOf({ kind: "done" }, ADDRESSES), null);
  assertEquals(transactionOf({ kind: "stuck", reason: "x" }, ADDRESSES), null);
  assertThrows(() => transactionOf({ kind: "mint", micros: 0n }, ADDRESSES));
  assertThrows(() =>
    transactionOf({ kind: "mint", micros: 10n ** 16n }, ADDRESSES)
  );
});
