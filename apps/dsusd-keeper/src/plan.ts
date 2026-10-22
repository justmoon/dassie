// What the keeper does next, from the ledger's state alone
// (spec/areas/dsusd-keeper.md#a-run). A run reads the state, takes the one
// step this returns, and reads again, until it is done: so a run that
// stopped halfway, or a testnet that reset, needs nothing remembered. The
// steps make the accounts, the line and the pool exist, hold the pool at
// the target with DSUSD alone, deepen it to the configured depth, and give
// back what DSUSD the keeper is left holding. None buys XRP.

import { dsusdAt, dsusdBeside, gap, type Pool, withdrawCap } from "./amm.ts";
import type { Account, Line } from "./state.ts";
import {
  DROPS_PER_XRP,
  DSUSD_DECIMALS,
  DSUSD_MICROS_MAX,
  formatUnits,
} from "./units.ts";

/** What the keeper reads before each step. */
export type State = {
  issuer: Account | null;
  keeper: Account | null;
  /** The keeper's DSUSD line; null without one, or without the keeper. */
  line: Line | null;
  pool: Pool | null;
};

export type Config = {
  /** The XRP the keeper deepens the pool to, in drops. */
  depthDrops: bigint;
  /** How far the pool's price may be from the target, in millionths,
   * before the keeper moves it. */
  bandPpm: bigint;
};

/** What a run may still spend: faucet requests, DSUSD minted, and
 * withdrawals. Each run starts with `BUDGET`. */
export type Budget = {
  faucetCalls: number;
  mintMicros: bigint;
  withdrawals: number;
};

/** One run's budget: a faucet request for each account and one for depth;
 * far more DSUSD than a run at the default depth mints, so that a ticker
 * or a pool misread can cost a bounded amount at most; and one withdrawal,
 * which is all a run needs while the keeper is the pool's main liquidity
 * provider. A keeper with a small share would otherwise withdraw again and
 * again within a run, each time selling the rest of its XRP in the pool
 * to the other providers; once a run, the failed runs show it instead. */
export const BUDGET: Budget = {
  faucetCalls: 3,
  mintMicros: 100_000n * 1_000_000n,
  withdrawals: 1,
};

/** The XRP the keeper keeps for its reserve (the account, its DSUSD line,
 * its LP tokens) and its fees, in drops. */
export const KEEPER_FLOOR_DROPS = 25n * DROPS_PER_XRP;
/** The least XRP a pool is made with, and the least a deposit for depth
 * adds, in drops: less is not worth a transaction. */
export const POOL_DROPS_MIN = 10n * DROPS_PER_XRP;
export const DEEPEN_DROPS_MIN = 10n * DROPS_PER_XRP;

export type Step =
  /** Fund an account from the faucet: one missing, or the keeper's XRP
   * for the pool. */
  | { kind: "fund"; account: "issuer" | "keeper" }
  /** Set DefaultRipple on the issuer, which a pool of its token needs. */
  | { kind: "default-ripple" }
  /** The keeper's line to the issuer, with NoRipple on its side. */
  | { kind: "trust-line" }
  /** The issuer pays the keeper new DSUSD. */
  | { kind: "mint"; micros: bigint }
  /** Create the pool, or fill one emptied, at the target. */
  | { kind: "create-pool"; drops: bigint; micros: bigint }
  | { kind: "refill-pool"; drops: bigint; micros: bigint }
  /** Single-asset deposit of DSUSD: XRP's price rises. */
  | { kind: "raise"; micros: bigint }
  /** Single-asset withdrawal of DSUSD: XRP's price falls. */
  | { kind: "lower"; micros: bigint }
  /** Two-asset deposit at the pool's ratio. */
  | { kind: "deepen"; drops: bigint; micros: bigint }
  /** The keeper pays DSUSD back to the issuer, which burns it. */
  | { kind: "burn"; micros: bigint }
  | { kind: "done" }
  /** Nothing the keeper may do helps; the run fails, and the next tries
   * again. */
  | { kind: "stuck"; reason: string };

/** The step after `state`, to hold the pool at `usdMicrosPerXrp`. */
export function nextStep(
  state: State,
  usdMicrosPerXrp: bigint,
  config: Config,
  budget: Budget,
): Step {
  const { issuer, keeper, line, pool } = state;
  if (issuer === null) return fund("issuer", budget);
  if (keeper === null) return fund("keeper", budget);
  if (!issuer.defaultRipple) return { kind: "default-ripple" };
  if (line === null || !line.limitOk || !line.noRipple) {
    return { kind: "trust-line" };
  }

  const held = line.balanceMicros > 0n ? line.balanceMicros : 0n;
  const spendable = keeper.drops > KEEPER_FLOOR_DROPS
    ? keeper.drops - KEEPER_FLOOR_DROPS
    : 0n;

  // No pool, or one whose liquidity was all withdrawn: make it at the
  // target, with as much of the depth as the keeper holds.
  const empty = pool !== null &&
    (pool.drops === 0n || pool.micros === 0n || pool.lpTotal.mantissa === 0n);
  if (pool === null || empty) {
    const drops = spendable < config.depthDrops ? spendable : config.depthDrops;
    if (drops < POOL_DROPS_MIN) return fund("keeper", budget);
    const micros = dsusdAt(drops, usdMicrosPerXrp);
    if (micros <= 0n || micros > DSUSD_MICROS_MAX) {
      return { kind: "stuck", reason: `a pool of ${micros} DSUSD millionths` };
    }
    if (held < micros) return mint(micros - held, budget);
    return {
      kind: pool === null ? "create-pool" : "refill-pool",
      drops,
      micros,
    };
  }

  // The price first: it is what nodes and market makers read.
  const toTarget = gap(pool, usdMicrosPerXrp, config.bandPpm);
  if (toTarget.side === "low") {
    if (held >= toTarget.micros) {
      return { kind: "raise", micros: toTarget.micros };
    }
    if (budget.mintMicros > 0n) return mint(toTarget.micros - held, budget);
    if (held > 0n) return { kind: "raise", micros: held };
    return { kind: "stuck", reason: "the run's DSUSD to mint is spent" };
  }
  if (toTarget.side === "high") {
    const cap = withdrawCap(pool);
    const micros = toTarget.micros < cap ? toTarget.micros : cap;
    if (micros > 0n && budget.withdrawals > 0) return { kind: "lower", micros };
    // The keeper's share cannot lower it further in this run. Depth and
    // the burn below still go on, and then the run is stuck.
  }

  // Then depth, with faucet XRP and DSUSD beside it at the pool's ratio.
  if (pool.drops + DEEPEN_DROPS_MIN <= config.depthDrops) {
    const missing = config.depthDrops - pool.drops;
    const drops = spendable < missing ? spendable : missing;
    if (drops >= DEEPEN_DROPS_MIN) {
      const micros = dsusdBeside(drops, pool);
      if (micros > DSUSD_MICROS_MAX) {
        return {
          kind: "stuck",
          reason: `a deposit of ${micros} DSUSD millionths`,
        };
      }
      if (held >= micros) return { kind: "deepen", drops, micros };
      if (budget.mintMicros >= micros - held) {
        return mint(micros - held, budget);
      }
    } else if (budget.faucetCalls > 0) {
      return { kind: "fund", account: "keeper" };
    }
  }

  // What DSUSD is left goes back to the issuer, so none is outstanding
  // but what the pool and its buyers hold.
  if (held > 0n) return { kind: "burn", micros: held };
  if (toTarget.side === "high") {
    return {
      kind: "stuck",
      reason:
        "XRP is too dear in the pool, and the keeper's share cannot lower it",
    };
  }
  return { kind: "done" };
}

function fund(account: "issuer" | "keeper", budget: Budget): Step {
  if (budget.faucetCalls <= 0) {
    return {
      kind: "stuck",
      reason:
        `the ${account} needs XRP and the run's faucet requests are spent`,
    };
  }
  return { kind: "fund", account };
}

function mint(micros: bigint, budget: Budget): Step {
  const capped = micros < budget.mintMicros ? micros : budget.mintMicros;
  if (capped <= 0n) {
    return { kind: "stuck", reason: "the run's DSUSD to mint is spent" };
  }
  return { kind: "mint", micros: capped };
}

/** A step in a few words, for the run's line. */
export function describe(step: Step): string {
  const xrp = (drops: bigint) => `${formatUnits(drops, 6)} XRP`;
  const dsusd = (micros: bigint) =>
    `${formatUnits(micros, DSUSD_DECIMALS)} DSUSD`;
  switch (step.kind) {
    case "fund":
      return `fund ${step.account}`;
    case "default-ripple":
    case "trust-line":
    case "done":
      return step.kind;
    case "mint":
    case "raise":
    case "lower":
    case "burn":
      return `${step.kind} ${dsusd(step.micros)}`;
    case "create-pool":
    case "refill-pool":
    case "deepen":
      return `${step.kind} ${xrp(step.drops)} + ${dsusd(step.micros)}`;
    case "stuck":
      return `stuck: ${step.reason}`;
  }
}

/** The budget left after `step`. */
export function spend(budget: Budget, step: Step): Budget {
  if (step.kind === "fund") {
    return { ...budget, faucetCalls: budget.faucetCalls - 1 };
  }
  if (step.kind === "mint") {
    return { ...budget, mintMicros: budget.mintMicros - step.micros };
  }
  if (step.kind === "lower") {
    return { ...budget, withdrawals: budget.withdrawals - 1 };
  }
  return budget;
}
