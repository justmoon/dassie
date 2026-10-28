// When a following keeper runs, on a clock the tests move.

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import {
  poolEvent,
  Schedule,
  TICK_MS,
  WINDOW_MS,
  WINDOWS_PER_LIFE,
} from "./follow.ts";
import { BUDGET, type Budget } from "./plan.ts";

const START = 1_000_000;
const OWN = new Set(["rKeeper", "rIssuer"]);

function spent(budget: Budget): Budget {
  return { ...budget, faucetCalls: budget.faucetCalls - 1, withdrawals: 0 };
}

test("a process runs at once, to catch up, and then once a minute", () => {
  const schedule = new Schedule(START);
  assert(schedule.start(START) !== null);
  assertEquals(schedule.start(START + 1), null);
  assertEquals(schedule.start(START + TICK_MS - 1), null);
  assertEquals(schedule.tickAt(), START + TICK_MS);
  assert(schedule.start(START + TICK_MS) !== null);
});

test("a pool event asks for a run at once", () => {
  const schedule = new Schedule(START);
  schedule.start(START);
  schedule.poolTouched(100);
  assert(schedule.start(START + 1) !== null);
  assertEquals(schedule.start(START + 2), null);
});

test("an event of a ledger a run already read asks for none", () => {
  const schedule = new Schedule(START);
  schedule.start(START);
  // Several trades in ledger 100: the first starts a run, which reads
  // ledger 100 or later.
  schedule.poolTouched(100);
  assert(schedule.start(START + 1) !== null);
  schedule.poolTouched(100);
  schedule.poolTouched(99);
  assertEquals(schedule.start(START + 2), null);
  // One of a later ledger, while that run was reading, asks again.
  schedule.poolTouched(101);
  assert(schedule.start(START + 3) !== null);
});

test("connecting again asks for a run", () => {
  const schedule = new Schedule(START);
  schedule.start(START);
  schedule.catchUp();
  assert(schedule.start(START + 1) !== null);
});

test("a window's budget is spent across its runs, and the next starts full", () => {
  const schedule = new Schedule(START);
  const first = schedule.start(START);
  assert(first !== null);
  assertEquals(first.budget, BUDGET);
  schedule.finish(first.window, spent(first.budget));

  schedule.poolTouched(100);
  const second = schedule.start(START + WINDOW_MS - 1);
  assert(second !== null);
  assertEquals(second.window, first.window);
  assertEquals(second.budget, spent(BUDGET));

  schedule.poolTouched(101);
  const third = schedule.start(START + WINDOW_MS);
  assert(third !== null);
  assertEquals(third.window, first.window + 1);
  assertEquals(third.budget, BUDGET);
});

test("a run that outlasts its window spends that window's budget only", () => {
  const schedule = new Schedule(START);
  const run = schedule.start(START + WINDOW_MS - 1);
  assert(run !== null);
  schedule.poolTouched(100);
  const next = schedule.start(START + WINDOW_MS);
  assert(next !== null);
  // The first run ends after the second started, in the next window.
  schedule.finish(run.window, spent(run.budget));
  schedule.finish(next.window, next.budget);
  schedule.poolTouched(101);
  assertEquals(schedule.start(START + WINDOW_MS + 1)?.budget, BUDGET);
});

test("a process ends after its life, or at the end of the window it failed in", () => {
  const schedule = new Schedule(START);
  assertEquals(
    schedule.endAt(START + 1, false),
    START + WINDOWS_PER_LIFE * WINDOW_MS,
  );
  assertEquals(schedule.endAt(START, true), START + WINDOW_MS);
  assertEquals(
    schedule.endAt(START + 2 * WINDOW_MS + 7, true),
    START + 3 * WINDOW_MS,
  );
  assertEquals(schedule.windowOf(START - 5), 0);
});

test("a validated transaction of someone else's is a pool event", () => {
  const trade = {
    type: "transaction",
    validated: true,
    ledger_index: 123,
    tx_json: { Account: "rTrader", TransactionType: "Payment" },
  };
  assertEquals(poolEvent(trade, OWN), 123);
  // API version 1 names it `transaction`.
  const { tx_json, ...rest } = trade;
  assertEquals(poolEvent({ ...rest, transaction: tx_json }, OWN), 123);
});

test("the keeper's own, unvalidated ones and other messages are not", () => {
  const base = { type: "transaction", validated: true, ledger_index: 123 };
  assertEquals(
    poolEvent({ ...base, tx_json: { Account: "rKeeper" } }, OWN),
    null,
  );
  assertEquals(
    poolEvent({ ...base, tx_json: { Account: "rIssuer" } }, OWN),
    null,
  );
  assertEquals(
    poolEvent({ ...base, validated: false, tx_json: { Account: "rT" } }, OWN),
    null,
  );
  assertEquals(poolEvent({ ...base, type: "ledgerClosed" }, OWN), null);
  assertEquals(poolEvent({ ...base, ledger_index: "123" }, OWN), null);
  assertEquals(poolEvent({ ...base, ledger_index: 0 }, OWN), null);
  assertEquals(poolEvent(null, OWN), null);
  // Without an account to tell, it counts: a run that finds the pool at
  // the target does nothing.
  assertEquals(poolEvent(base, OWN), 123);
});
