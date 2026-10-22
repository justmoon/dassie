// The plan against a model of the ledger: each step applied as the ledger
// would, with the model checking what the ledger checks (enough XRP, enough
// DSUSD, enough LP tokens), and traders moving the pool in between.

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import {
  dsusdAt,
  FEE_UNITS_PER_ONE,
  gap,
  type Pool,
  withdrawCap,
} from "./amm.ts";
import {
  BUDGET,
  type Budget,
  type Config,
  DEEPEN_DROPS_MIN,
  describe,
  KEEPER_FLOOR_DROPS,
  nextStep,
  spend,
  type State,
  type Step,
} from "./plan.ts";
import { TRADING_FEE_UNITS, transactionOf } from "./transactions.ts";
import type { Decimal } from "./units.ts";

const XRP = 1_000_000n;
const DSUSD = 1_000_000n;
/** What the testnet faucet hands out per request. */
const FAUCET_DROPS = 100n * XRP;
/** AMMCreate's fee: one owner reserve. */
const AMM_CREATE_FEE_DROPS = 200_000n;
const PRICE = 2_500_000n;
const CONFIG: Config = { depthDrops: 1_000n * XRP, bandPpm: 2_000n };
/** The steps a run takes at most, as main.ts. */
const STEPS_MAX = 16;
const ADDRESSES = { issuer: "rIssuer", keeper: "rKeeper" };

const EMPTY: State = { issuer: null, keeper: null, line: null, pool: null };

const whole = (n: bigint): Decimal => ({ mantissa: n, exponent: 0 });
const lp = (d: Decimal): bigint => {
  assertEquals(d.exponent, 0);
  return d.mantissa;
};

function isqrt(n: bigint): bigint {
  if (n < 2n) return n;
  let x = n;
  let y = (x + 1n) / 2n;
  while (y < x) {
    x = y;
    y = (x + n / x) / 2n;
  }
  return x;
}

/** The LP tokens the ledger burns for a single-asset withdrawal of `b` of
 * `balance`, rounded up: T·x, x the smaller root of x² − c·x + r = 0. */
function lpBurned(pool: Pool, b: bigint): bigint {
  const S = 10n ** 18n;
  const r = (b * S + pool.micros - 1n) / pool.micros;
  const f = (BigInt(pool.feeUnits) * S) / FEE_UNITS_PER_ONE;
  const c = (r * f) / S + 2n * S - f;
  const x = (c - isqrt(c * c - 4n * r * S) + 1n) / 2n;
  return (lp(pool.lpTotal) * x + S - 1n) / S;
}

/** `step` applied to `state` as the ledger would, asserting what it
 * checks. Swaps and other LPs go through `trade` and `share`. */
function apply(state: State, step: Step): State {
  const { issuer, keeper, line, pool } = state;
  switch (step.kind) {
    case "fund": {
      const account = state[step.account] ??
        { drops: 0n, defaultRipple: false };
      return {
        ...state,
        [step.account]: { ...account, drops: account.drops + FAUCET_DROPS },
      };
    }
    case "default-ripple":
      assert(issuer !== null);
      return { ...state, issuer: { ...issuer, defaultRipple: true } };
    case "trust-line":
      assert(keeper !== null && keeper.drops >= KEEPER_FLOOR_DROPS);
      return {
        ...state,
        line: {
          balanceMicros: line?.balanceMicros ?? 0n,
          limitOk: true,
          noRipple: true,
        },
      };
    case "mint":
      assert(issuer !== null && issuer.defaultRipple && line !== null);
      return {
        ...state,
        line: { ...line, balanceMicros: line.balanceMicros + step.micros },
      };
    case "burn":
      assert(line !== null && line.balanceMicros >= step.micros);
      return {
        ...state,
        line: { ...line, balanceMicros: line.balanceMicros - step.micros },
      };
    case "create-pool":
    case "refill-pool": {
      assert(keeper !== null && line !== null);
      assert(step.kind === "create-pool" ? pool === null : pool !== null);
      assert(
        issuer?.defaultRipple === true,
        "a pool of a token needs DefaultRipple",
      );
      assert(line.balanceMicros >= step.micros, "the keeper holds the DSUSD");
      const fee = step.kind === "create-pool" ? AMM_CREATE_FEE_DROPS : 0n;
      assert(keeper.drops - step.drops - fee >= KEEPER_FLOOR_DROPS - 1n * XRP);
      const tokens = isqrt(step.drops * step.micros);
      return {
        issuer,
        keeper: { ...keeper, drops: keeper.drops - step.drops - fee },
        line: { ...line, balanceMicros: line.balanceMicros - step.micros },
        pool: {
          drops: step.drops,
          micros: step.micros,
          lpTotal: whole(tokens),
          lpKeeper: whole(tokens),
          feeUnits: TRADING_FEE_UNITS,
        },
      };
    }
    case "raise": {
      assert(
        pool !== null && line !== null && line.balanceMicros >= step.micros,
      );
      // Fewer LP tokens than the share, for the fee; the model only needs
      // them positive.
      const tokens = (lp(pool.lpTotal) * step.micros) /
        (2n * pool.micros + 2n * step.micros);
      return {
        ...state,
        line: { ...line, balanceMicros: line.balanceMicros - step.micros },
        pool: {
          ...pool,
          micros: pool.micros + step.micros,
          lpTotal: whole(lp(pool.lpTotal) + tokens),
          lpKeeper: whole(lp(pool.lpKeeper) + tokens),
        },
      };
    }
    case "lower": {
      assert(pool !== null && line !== null && step.micros < pool.micros);
      const burned = lpBurned(pool, step.micros);
      assert(burned <= lp(pool.lpKeeper), "a withdrawal within the share");
      return {
        ...state,
        line: { ...line, balanceMicros: line.balanceMicros + step.micros },
        pool: {
          ...pool,
          micros: pool.micros - step.micros,
          lpTotal: whole(lp(pool.lpTotal) - burned),
          lpKeeper: whole(lp(pool.lpKeeper) - burned),
        },
      };
    }
    case "deepen": {
      assert(pool !== null && keeper !== null && line !== null);
      const used = (step.drops * pool.micros + pool.drops - 1n) / pool.drops;
      assert(used <= step.micros && line.balanceMicros >= step.micros);
      assert(keeper.drops - step.drops >= KEEPER_FLOOR_DROPS);
      const tokens = (lp(pool.lpTotal) * step.drops) / pool.drops;
      return {
        issuer,
        keeper: { ...keeper, drops: keeper.drops - step.drops },
        line: { ...line, balanceMicros: line.balanceMicros - used },
        pool: {
          ...pool,
          drops: pool.drops + step.drops,
          micros: pool.micros + used,
          lpTotal: whole(lp(pool.lpTotal) + tokens),
          lpKeeper: whole(lp(pool.lpKeeper) + tokens),
        },
      };
    }
    case "done":
    case "stuck":
      throw new Error(`${step.kind} is not applied`);
  }
}

/** A trader swaps through the pool: XRP in (DSUSD out) when `drops` is
 * positive, DSUSD in (XRP out) when `micros` is. No fee: the model needs
 * only the pool moved. */
function trade(state: State, { drops = 0n, micros = 0n }): State {
  const pool = state.pool;
  assert(pool !== null);
  const k = pool.drops * pool.micros;
  if (drops > 0n) {
    const after = pool.drops + drops;
    return {
      ...state,
      pool: { ...pool, drops: after, micros: (k + after - 1n) / after },
    };
  }
  const after = pool.micros + micros;
  return {
    ...state,
    pool: { ...pool, micros: after, drops: (k + after - 1n) / after },
  };
}

type Run = {
  state: State;
  steps: string[];
  last: Step;
  minted: bigint;
  faucet: number;
};

/** One run, as main.ts takes it, against the model. */
function run(
  start: State,
  price: bigint,
  config = CONFIG,
  budget: Budget = BUDGET,
): Run {
  let state = start;
  let left = budget;
  const steps: string[] = [];
  let minted = 0n;
  let faucet = 0;
  for (let i = 0; i < STEPS_MAX; i += 1) {
    const step = nextStep(state, price, config, left);
    if (step.kind === "done" || step.kind === "stuck") {
      return { state, steps, last: step, minted, faucet };
    }
    checkTransaction(step);
    left = spend(left, step);
    if (step.kind === "mint") minted += step.micros;
    if (step.kind === "fund") faucet += 1;
    state = apply(state, step);
    steps.push(step.kind);
  }
  throw new Error(`no end after ${STEPS_MAX} steps: ${steps.join(", ")}`);
}

/** No step pays the keeper XRP from anyone: it never buys XRP. */
function checkTransaction(step: Step): void {
  const submission = transactionOf(step, ADDRESSES);
  if (submission === null) {
    assertEquals(step.kind, "fund");
    return;
  }
  const { tx } = submission;
  assert(
    [
      "AccountSet",
      "TrustSet",
      "Payment",
      "AMMCreate",
      "AMMDeposit",
      "AMMWithdraw",
    ]
      .includes(tx.TransactionType),
  );
  if (
    tx.TransactionType === "Payment" || tx.TransactionType === "AMMWithdraw"
  ) {
    assertEquals(
      typeof tx["Amount"],
      "object",
      `${tx.TransactionType} pays DSUSD only`,
    );
    assertEquals(tx["Amount2"], undefined);
  }
}

function assertSettled(state: State, price: bigint): void {
  assert(state.pool !== null && state.line !== null);
  assertEquals(gap(state.pool, price, CONFIG.bandPpm), { side: "in-band" });
  assertEquals(
    state.line.balanceMicros,
    0n,
    "the keeper holds no DSUSD after a run",
  );
}

test("a fresh testnet: accounts, line and pool made, at the target", () => {
  const result = run(EMPTY, PRICE);
  assertEquals(result.steps, [
    "fund",
    "fund",
    "default-ripple",
    "trust-line",
    "mint",
    "create-pool",
    "fund",
    "mint",
    "deepen",
  ]);
  assertEquals(result.last.kind, "done");
  assertSettled(result.state, PRICE);
  assert(result.faucet <= BUDGET.faucetCalls);
});

test("the pool deepens run by run to the configured depth, then stops", () => {
  let state = run(EMPTY, PRICE).state;
  for (let i = 0; i < 10; i += 1) {
    const result = run(state, PRICE);
    assertEquals(result.last.kind, "done");
    state = result.state;
    assertSettled(state, PRICE);
  }
  assert(state.pool !== null);
  assert(state.pool.drops + DEEPEN_DROPS_MIN > CONFIG.depthDrops);
  assert(state.pool.drops <= CONFIG.depthDrops);
  assertEquals(run(state, PRICE).steps, []);
});

/** A pool at depth and at the target. */
function settled(): State {
  let state = EMPTY;
  for (let i = 0; i < 10; i += 1) state = run(state, PRICE).state;
  return state;
}

test("a buyer of DSUSD lowers XRP's price; the keeper mints and raises it", () => {
  const bought = trade(settled(), { drops: 50n * XRP });
  const result = run(bought, PRICE);
  assertEquals(result.steps, ["mint", "raise"]);
  assertSettled(result.state, PRICE);
});

test("a seller of DSUSD raises XRP's price; the keeper withdraws and burns", () => {
  const sold = trade(settled(), { micros: 100n * DSUSD });
  const result = run(sold, PRICE);
  // The seller took XRP out, so the keeper deepens the pool again with the
  // DSUSD it withdrew, and burns the rest.
  assertEquals(result.steps, ["lower", "deepen", "burn"]);
  assertEquals(result.minted, 0n);
  assertSettled(result.state, PRICE);
});

test("the market moves; the pool follows with DSUSD alone", () => {
  const start = settled();
  const up = run(start, (PRICE * 11n) / 10n);
  assertEquals(up.steps, ["mint", "raise"]);
  assertSettled(up.state, (PRICE * 11n) / 10n);
  assertEquals(up.state.pool?.drops, start.pool?.drops);
  const down = run(start, (PRICE * 9n) / 10n);
  assertEquals(down.steps, ["lower", "burn"]);
  assertSettled(down.state, (PRICE * 9n) / 10n);
  assertEquals(down.state.pool?.drops, start.pool?.drops);
});

test("within the band, nothing is done", () => {
  const start = settled();
  assertEquals(run(start, PRICE + PRICE / 1_000n).steps, []);
});

test("a withdrawal never exceeds the keeper's share of the pool", () => {
  const start = settled();
  assert(start.pool !== null);
  // Someone else deposited nine times what the keeper holds.
  const total = lp(start.pool.lpTotal);
  const crowded: State = {
    ...start,
    pool: {
      ...start.pool,
      drops: start.pool.drops * 10n,
      micros: start.pool.micros * 10n,
      lpTotal: whole(total * 10n),
    },
  };
  const price = (PRICE * 7n) / 10n;
  const step = nextStep(crowded, price, CONFIG, BUDGET);
  assert(step.kind === "lower" && crowded.pool !== null);
  const toTarget = gap(crowded.pool, price, CONFIG.bandPpm);
  assert(toTarget.side === "high" && step.micros < toTarget.micros);
  assertEquals(step.micros, withdrawCap(crowded.pool));
  // The model asserts the LP tokens burned are within the keeper's; one
  // withdrawal a run, after which the run is stuck rather than sell the
  // rest of the keeper's share.
  const result = run(crowded, price);
  assertEquals(result.steps.filter((kind) => kind === "lower"), ["lower"]);
  assertEquals(result.last.kind, "stuck");
});

test("a pool the keeper holds none of, priced too high, leaves the run stuck", () => {
  const start = settled();
  assert(start.pool !== null);
  const foreign: State = {
    ...start,
    pool: { ...start.pool, lpKeeper: whole(0n) },
  };
  const result = run(foreign, (PRICE * 9n) / 10n);
  assertEquals(result.last.kind, "stuck");
});

test("a run mints at most its budget", () => {
  const start = settled();
  const budget = { ...BUDGET, mintMicros: 10n * DSUSD };
  const result = run(start, PRICE * 2n, CONFIG, budget);
  assert(result.minted <= budget.mintMicros);
  assertEquals(result.last.kind, "stuck");
  assertEquals(
    describe(result.last),
    "stuck: the run's DSUSD to mint is spent",
  );
});

test("without faucet requests left, a missing account leaves the run stuck", () => {
  const result = run(EMPTY, PRICE, CONFIG, { ...BUDGET, faucetCalls: 0 });
  assertEquals(result.steps, []);
  assertEquals(result.last.kind, "stuck");
});

test("after a testnet reset, everything is made again", () => {
  // A reset takes the accounts, the line and the pool; the seeds give
  // the same addresses, and the next run starts from nothing.
  assertEquals(run(settled(), PRICE).steps, []);
  const after = run(EMPTY, PRICE);
  assertEquals(after.steps, run(EMPTY, PRICE).steps);
  assertEquals(after.last.kind, "done");
  assertSettled(after.state, PRICE);
});

test("a line made otherwise is set again", () => {
  const start = settled();
  assert(start.line !== null);
  const rippling = { ...start, line: { ...start.line, noRipple: false } };
  assertEquals(nextStep(rippling, PRICE, CONFIG, BUDGET).kind, "trust-line");
  const low = { ...start, line: { ...start.line, limitOk: false } };
  assertEquals(nextStep(low, PRICE, CONFIG, BUDGET).kind, "trust-line");
});

test("an emptied pool is filled again at the target", () => {
  const start = settled();
  assert(start.pool !== null && start.keeper !== null);
  const emptied: State = {
    ...start,
    keeper: { ...start.keeper, drops: start.keeper.drops + start.pool.drops },
    pool: {
      ...start.pool,
      drops: 0n,
      micros: 0n,
      lpTotal: whole(0n),
      lpKeeper: whole(0n),
    },
  };
  const step = nextStep(emptied, PRICE, CONFIG, BUDGET);
  assertEquals(step.kind, "mint");
  const minted = apply(emptied, step);
  const refill = nextStep(minted, PRICE, CONFIG, BUDGET);
  assertEquals(refill.kind, "refill-pool");
  assert(refill.kind === "refill-pool");
  assertEquals(refill.micros, dsusdAt(refill.drops, PRICE));
});

test("random markets: every run ends settled, within its budget, never buying XRP", () => {
  let seed = 0x9e37_79b9_7f4a_7c15n;
  const mask = (1n << 64n) - 1n;
  const next = (below: bigint) => {
    seed ^= seed >> 12n;
    seed ^= (seed << 25n) & mask;
    seed ^= seed >> 27n;
    return ((seed * 0x2545f4914f6cdd1dn) & mask) % below;
  };
  let state = settled();
  let price = PRICE;
  for (let i = 0; i < 300; i += 1) {
    // The market moves up to 5% either way, and traders swap.
    price = (price * (950n + next(101n))) / 1_000n;
    if (next(2n) === 0n) state = trade(state, { drops: 1n + next(200n * XRP) });
    else state = trade(state, { micros: 1n + next(400n * DSUSD) });
    const result = run(state, price);
    assertEquals(result.last.kind, "done", result.steps.join(", "));
    assert(result.minted <= BUDGET.mintMicros);
    assert(result.faucet <= BUDGET.faucetCalls);
    state = result.state;
    assertSettled(state, price);
  }
});
