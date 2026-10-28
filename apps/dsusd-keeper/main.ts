// The DSUSD keeper (spec/areas/dsusd-keeper.md,
// spec/decisions/dsusd.md): holds the XRP Ledger testnet's XRP/DSUSD pool
// at the exchange's XRP/USD price.
//
//   node apps/dsusd-keeper/main.ts [--once] [--dry-run]
//
// A run reads the ticker, then reads the ledger and takes the plan's next
// step (src/plan.ts), again and again, until the plan is done or a bound
// is reached. By default the keeper follows the pool: it runs at once
// after someone else's transaction touched the pool, and otherwise once a
// minute, for the market's moves, with a budget per window of time
// (src/follow.ts); it prints a line for each run that did something or
// could not, and one for each window. After its life it exits, and
// systemd starts it again from the newest `main` (vm/). With `--once` it
// runs once, prints one line and exits; `--dry-run` runs once, prints the
// first step and submits nothing.
//
// It reads its settings from the environment (src/config.ts): the
// issuer's and the keeper's seeds, which on the VM come from Secret
// Manager (vm/run.sh).

import process from "node:process";
import { parseArgs } from "node:util";
import { Wallet } from "xrpl";
import { midPrice, type Pool, swapOut } from "./src/amm.ts";
import { type Settings, settingsFrom } from "./src/config.ts";
import {
  poolEvent,
  Schedule,
  WINDOW_MS,
  WINDOWS_PER_LIFE,
} from "./src/follow.ts";
import { getJson, Ledger, type Listeners } from "./src/ledger.ts";
import { BUDGET, type Budget, describe, nextStep, spend } from "./src/plan.ts";
import { targetPrice, TICKER_URL } from "./src/price.ts";
import { type Addresses, transactionOf } from "./src/transactions.ts";
import { DROPS_PER_XRP, formatUnits, type Result } from "./src/units.ts";

/** A run's bounds: the steps it takes, and its time, within a window. */
const STEPS_MAX = 16;
const RUN_TIMEOUT_MS = 4 * 60_000;
/** How long a following keeper waits before it connects again, after it
 * could not. */
const RECONNECT_MS = 15_000;

type Keeper = {
  settings: Settings;
  wallets: { issuer: Wallet; keeper: Wallet };
  addresses: Addresses;
  who: string;
};

/** How a run ended: done (or, dry, decided), stuck, failed, or out of
 * steps; with what its budget has left, how many steps it took, its line,
 * and for one that did not get done, why in a few words. */
type Outcome = {
  kind: "done" | "stuck" | "failed" | "bounded";
  budget: Budget;
  steps: number;
  line: string;
  reason: string;
};

function keeperFrom(env: Record<string, string | undefined>): Result<Keeper> {
  const settings = settingsFrom(env);
  if (!settings.ok) return settings;
  const issuer = walletOf(settings.value.issuerSeed, "DSUSD_ISSUER_SEED");
  if (!issuer.ok) return issuer;
  const keeper = walletOf(settings.value.keeperSeed, "DSUSD_KEEPER_SEED");
  if (!keeper.ok) return keeper;
  const addresses = {
    issuer: issuer.value.classicAddress,
    keeper: keeper.value.classicAddress,
  };
  return {
    ok: true,
    value: {
      settings: settings.value,
      wallets: { issuer: issuer.value, keeper: keeper.value },
      addresses,
      who: `issuer ${addresses.issuer}, keeper ${addresses.keeper}`,
    },
  };
}

/** One run on `ledger`, spending at most `budget`. */
async function run(
  keeper: Keeper,
  ledger: Ledger,
  budget: Budget,
  dryRun: boolean,
): Promise<Outcome> {
  const { settings, wallets, addresses, who } = keeper;
  const failed = (reason: string): Outcome => ({
    kind: "failed",
    budget,
    steps: 0,
    line: `${reason}; ${who}`,
    reason,
  });
  const ticker = await getJson(TICKER_URL);
  if (!ticker.ok) return failed(ticker.error);
  const target = targetPrice(ticker.value, Date.now());
  if (!target.ok) return failed(target.error);
  const price = target.value.usdMicrosPerXrp;

  const done: string[] = [];
  const notes: string[] = [];
  try {
    for (let index = 0; index < STEPS_MAX; index += 1) {
      const state = await ledger.read(addresses);
      const step = nextStep(state, price, settings, budget);
      if (dryRun || step.kind === "done" || step.kind === "stuck") {
        const parts = [
          `target ${formatUnits(price, 6)} USD/XRP`,
          poolLine(state.pool),
          done.length === 0 ? "nothing done" : done.join(", "),
          ...(dryRun || step.kind === "stuck"
            ? [`next: ${describe(step)}`]
            : []),
          ...notes,
          who,
        ];
        return {
          kind: step.kind === "stuck" ? "stuck" : "done",
          budget,
          steps: done.length,
          line: parts.join("; "),
          reason: step.kind === "stuck" ? describe(step) : "",
        };
      }
      budget = spend(budget, step);
      if (step.kind === "fund") {
        try {
          await ledger.fund(settings.faucet, addresses[step.account]);
        } catch (error) {
          // The faucet is best effort: without it the run does what it can,
          // and is stuck only if it needed the XRP.
          notes.push(String(error instanceof Error ? error.message : error));
          budget = { ...budget, faucetCalls: 0 };
          continue;
        }
      } else {
        const submission = transactionOf(step, addresses);
        if (submission === null) {
          throw new Error(`${step.kind} submits nothing`);
        }
        await ledger.submit(submission, wallets[submission.signer]);
      }
      done.push(describe(step));
    }
    return {
      kind: "bounded",
      budget,
      steps: done.length,
      line: `target ${
        formatUnits(price, 6)
      } USD/XRP; ${STEPS_MAX} steps and not done: ${done.join(", ")}; ${who}`,
      reason: `${STEPS_MAX} steps and not done`,
    };
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    const steps = done.length === 0 ? "" : ` after ${done.join(", ")}`;
    return {
      kind: "failed",
      budget,
      steps: done.length,
      line: `failed${steps}: ${message}; ${who}`,
      reason: message,
    };
  }
}

/** One run with a fresh budget, and its line. */
async function once(dryRun: boolean): Promise<{ line: string; ok: boolean }> {
  const keeper = keeperFrom(process.env);
  if (!keeper.ok) return { line: keeper.error, ok: false };
  let ledger: Ledger;
  try {
    ledger = await Ledger.open(keeper.value.settings.server);
  } catch (error) {
    return { line: `${message(error)}; ${keeper.value.who}`, ok: false };
  }
  try {
    const outcome = await run(keeper.value, ledger, BUDGET, dryRun);
    return { line: outcome.line, ok: outcome.kind === "done" };
  } finally {
    await ledger.close();
  }
}

/** Follows the pool for the process's life; the exit code. */
async function follow(): Promise<number> {
  const found = keeperFrom(process.env);
  if (!found.ok) {
    say(found.error);
    return 1;
  }
  const keeper = found.value;
  const schedule = new Schedule(Date.now());
  const own = new Set([keeper.addresses.issuer, keeper.addresses.keeper]);
  let wake = () => {};
  let lost = false;
  const listeners: Listeners = {
    transaction: (message) => {
      const ledgerIndex = poolEvent(message, own);
      if (ledgerIndex === null) return;
      schedule.poolTouched(ledgerIndex);
      wake();
    },
    lost: () => {
      lost = true;
      wake();
    },
  };
  const sleep = (ms: number) =>
    new Promise<void>((resolve) => {
      const timer = setTimeout(done, ms > 0 ? ms : 0);
      function done() {
        clearTimeout(timer);
        wake = () => {};
        resolve();
      }
      wake = done;
    });

  let ledger: Ledger | null = null;
  // What the window so far did, for its line, and the last trouble said,
  // so that one which repeats run after run is said once.
  let window = 0;
  let runs = 0;
  let steps = 0;
  let last = "no run yet";
  let trouble = "";
  const troubled = (reason: string, line: string) => {
    if (reason !== trouble) say(line);
    trouble = reason;
  };
  try {
    for (;;) {
      const now = Date.now();
      if (schedule.windowOf(now) !== window) {
        say(
          `following ${
            ledger?.poolAccount ?? "no pool"
          }: ${runs} runs, ${steps} steps in the window; last: ${last}`,
        );
        window = schedule.windowOf(now);
        runs = 0;
        steps = 0;
      }
      const end = schedule.endAt(now, false);
      if (now >= end) return 0;

      if (ledger === null || lost) {
        if (ledger !== null) await closeQuietly(ledger);
        ledger = null;
        lost = false;
        try {
          ledger = await Ledger.open(keeper.settings.server, listeners);
          schedule.catchUp();
        } catch (error) {
          troubled(
            `connect: ${message(error)}`,
            `cannot connect: ${message(error)}`,
          );
          await sleep(Math.min(RECONNECT_MS, end - now));
          continue;
        }
      }

      const started = schedule.start(now);
      if (started === null) {
        await sleep(Math.min(schedule.tickAt(), end) - now);
        continue;
      }
      const outcome = await withTimeout(
        run(keeper, ledger, started.budget, false),
        RUN_TIMEOUT_MS,
      );
      if (outcome === null) {
        throw new Error(`a run had no end after ${RUN_TIMEOUT_MS} ms`);
      }
      schedule.finish(started.window, outcome.budget);
      runs += 1;
      steps += outcome.steps;
      last = outcome.line;
      if (outcome.kind === "done") {
        if (outcome.steps > 0) say(outcome.line);
        trouble = "";
      } else {
        troubled(outcome.reason, outcome.line);
      }
      const pool = ledger.poolAccount;
      if (pool !== null && !lost) {
        try {
          await ledger.follow(pool);
        } catch (error) {
          troubled(
            `follow: ${message(error)}`,
            `cannot follow ${pool}: ${message(error)}`,
          );
          lost = true;
        }
      }
    }
  } catch (error) {
    // Something the loop does not expect: the process ends, at the end of
    // this window, and systemd starts it again.
    const failedAt = Date.now();
    say(`failed: ${message(error)}; ${keeper.who}`);
    await sleep(schedule.endAt(failedAt, true) - failedAt);
    return 1;
  } finally {
    if (ledger !== null) await closeQuietly(ledger);
  }
}

async function closeQuietly(ledger: Ledger): Promise<void> {
  try {
    await ledger.close();
  } catch (_error) {
    // A connection already gone has nothing to close.
  }
}

/** `promise`'s value, or null once `ms` passed. */
async function withTimeout<T>(
  promise: Promise<T>,
  ms: number,
): Promise<T | null> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<null>((resolve) => {
    timer = setTimeout(() => resolve(null), ms);
  });
  try {
    return await Promise.race([promise, timeout]);
  } finally {
    clearTimeout(timer);
  }
}

/** The wallet of `seed`. A seed that is none is refused by its setting's
 * name alone: xrpl.js's message can quote the seed's letters, which must
 * not reach the log. */
function walletOf(seed: string, name: string): Result<Wallet> {
  try {
    return { ok: true, value: Wallet.fromSeed(seed) };
  } catch (_error) {
    return { ok: false, error: `${name}: not a seed` };
  }
}

/** The pool as the run left it: its two sides, its mid price, and what
 * one XRP buys, after the trading fee. */
function poolLine(pool: Pool | null): string {
  if (pool === null || pool.drops === 0n || pool.micros === 0n) {
    return "no pool";
  }
  const buys = swapOut(DROPS_PER_XRP, pool.drops, pool.micros, pool.feeUnits);
  return `pool ${formatUnits(pool.drops, 6)} XRP / ${
    formatUnits(pool.micros, 6)
  } DSUSD at ${formatUnits(midPrice(pool), 6)} USD/XRP, 1 XRP buys ${
    formatUnits(buys, 6)
  } DSUSD`;
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function say(line: string): void {
  console.log(`dsusd-keeper: ${line}`);
}

const { values } = parseArgs({
  options: { once: { type: "boolean" }, "dry-run": { type: "boolean" } },
});
const single = values.once === true || values["dry-run"] === true;
// A bound on the whole process: a run's timeout, or its life and the
// window a failure waits out, and a minute for closing.
const boundMs = single
  ? RUN_TIMEOUT_MS
  : (WINDOWS_PER_LIFE + 1) * WINDOW_MS + 60_000;
const watchdog = setTimeout(() => {
  say(`no end after ${boundMs} ms`);
  process.exit(1);
}, boundMs);
try {
  if (single) {
    const { line, ok } = await once(values["dry-run"] === true);
    say(line);
    process.exitCode = ok ? 0 : 1;
  } else {
    process.exitCode = await follow();
  }
} catch (error) {
  say(`failed: ${message(error)}`);
  process.exitCode = 1;
} finally {
  clearTimeout(watchdog);
}
// A run that failed early can leave a connection or a request open, which
// would keep Node running after its line until systemd stops it, and hold
// back its next start.
process.exit();
