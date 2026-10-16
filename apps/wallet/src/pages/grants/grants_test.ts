import { test } from "node:test";
import { assertEquals } from "@std/assert";
import {
  accessText,
  clientName,
  intervalText,
  limitText,
  spentText,
} from "./grants.ts";

const usd = (value: bigint) => ({ value, assetCode: "USD", assetScale: 9 });

test("intervals read as people say them", () => {
  assertEquals(intervalText(""), undefined);
  assertEquals(intervalText("R/2024-01-01T00:00:00Z/P1D"), "a day");
  assertEquals(intervalText("R12/2024-01-01T00:00:00Z/P1M"), "a month");
  assertEquals(intervalText("R/2024-01-01T00:00:00Z/P2W"), "every 2 weeks");
  assertEquals(
    intervalText("R-1/P1M10DT2H30M/2024-01-01T00:00:00Z"),
    "every 1 month and 10 days and 2 hours and 30 minutes",
  );
  assertEquals(
    intervalText("R/2024-01-01T00:00:00Z/2024-02-01T00:00:00Z"),
    "each period",
  );
});

test("limits read as a sentence", () => {
  const limits = {
    receiver: "",
    interval: "R/2024-01-01T00:00:00Z/P1D",
    debitAmount: usd(10_000_000_000n),
    spentDebit: 2_500_000_000n,
    spentReceive: 0n,
  };
  assertEquals(limitText(limits), " of up to $ 10.00 a day");
  assertEquals(
    accessText({
      type: "outgoing-payment",
      actions: ["create", "read"],
      ledger: "stub+usd",
      limits,
    }),
    "Send payments of up to $ 10.00 a day from your USD wallet",
  );
  assertEquals(spentText(limits), "$ 2.50 of $ 10.00 spent this period");
  assertEquals(
    limitText({ ...limits, interval: "", debitAmount: undefined }),
    " of any amount",
  );
});

test("a limit on what is delivered says so", () => {
  const limits = {
    receiver: "",
    interval: "",
    receiveAmount: usd(5_000_000_000n),
    spentDebit: 0n,
    spentReceive: 1_000_000_000n,
  };
  assertEquals(limitText(limits), " delivering up to $ 5.00");
  assertEquals(spentText(limits), "$ 1.00 of $ 5.00 delivered");
});

test("an item that only reads shows only its receiver", () => {
  const limits = {
    receiver: "https://example.com/incoming-payments/1",
    interval: "",
    spentDebit: 0n,
    spentReceive: 0n,
  };
  assertEquals(
    accessText({
      type: "outgoing-payment",
      actions: ["read"],
      ledger: "stub+usd",
      limits,
    }),
    "See outgoing payments, to one payment request only from your USD wallet",
  );
});

test("reading all of a kind says so", () => {
  assertEquals(
    accessText({ type: "quote", actions: ["read-all"], ledger: "" }),
    "See quotes, and see all of them, not only its own",
  );
});

test("a client goes by its public name, else its address", () => {
  assertEquals(
    clientName("https://app.example/me", "Budget App"),
    "Budget App",
  );
  assertEquals(clientName("https://app.example/me", ""), "app.example/me");
});
