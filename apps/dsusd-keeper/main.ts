// The DSUSD keeper (spec/areas/dsusd-keeper.md,
// spec/decisions/dsusd.md): one run, which holds the XRP Ledger testnet's
// XRP/DSUSD pool at the exchange's XRP/USD price.
//
//   node apps/dsusd-keeper/main.ts [--dry-run]
//
// A run reads the ticker, then reads the ledger and takes the plan's next
// step (src/plan.ts), again and again, until the plan is done or a bound
// is reached, and prints one line. A run that fails exits with an error;
// the next run, which a systemd timer starts on the keeper's VM
// (vm/dsusd-keeper.timer), starts again from what the ledger holds. With
// `--dry-run` it prints the first step and submits nothing.
//
// It reads its settings from the environment (src/config.ts): the
// issuer's and the keeper's seeds, which on the VM come from Secret
// Manager (vm/run.sh).

import process from "node:process";
import { parseArgs } from "node:util";
import { Wallet } from "xrpl";
import { midPrice, type Pool, swapOut } from "./src/amm.ts";
import { settingsFrom } from "./src/config.ts";
import { getJson, Ledger } from "./src/ledger.ts";
import { BUDGET, describe, nextStep, spend } from "./src/plan.ts";
import { targetPrice, TICKER_URL } from "./src/price.ts";
import { transactionOf } from "./src/transactions.ts";
import { DROPS_PER_XRP, formatUnits, type Result } from "./src/units.ts";

/** A run's bounds: the steps it takes, and its time, within the timer's
 * period, so that runs never overlap. */
const STEPS_MAX = 16;
const RUN_TIMEOUT_MS = 4 * 60_000;

async function run(dryRun: boolean): Promise<{ line: string; ok: boolean }> {
  const settings = settingsFrom(process.env);
  if (!settings.ok) return { line: settings.error, ok: false };
  const issuer = walletOf(settings.value.issuerSeed, "DSUSD_ISSUER_SEED");
  if (!issuer.ok) return { line: issuer.error, ok: false };
  const keeper = walletOf(settings.value.keeperSeed, "DSUSD_KEEPER_SEED");
  if (!keeper.ok) return { line: keeper.error, ok: false };
  const wallets = { issuer: issuer.value, keeper: keeper.value };
  const addresses = {
    issuer: wallets.issuer.classicAddress,
    keeper: wallets.keeper.classicAddress,
  };
  const who = `issuer ${addresses.issuer}, keeper ${addresses.keeper}`;

  const ticker = await getJson(TICKER_URL);
  if (!ticker.ok) return { line: `${ticker.error}; ${who}`, ok: false };
  const target = targetPrice(ticker.value, Date.now());
  if (!target.ok) return { line: `${target.error}; ${who}`, ok: false };
  const price = target.value.usdMicrosPerXrp;

  const ledger = await Ledger.open(settings.value.server);
  const done: string[] = [];
  const notes: string[] = [];
  try {
    let budget = BUDGET;
    for (let index = 0; index < STEPS_MAX; index += 1) {
      const state = await ledger.read(addresses);
      const step = nextStep(state, price, settings.value, budget);
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
        return { line: parts.join("; "), ok: step.kind !== "stuck" };
      }
      budget = spend(budget, step);
      if (step.kind === "fund") {
        try {
          await ledger.fund(settings.value.faucet, addresses[step.account]);
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
      line: `target ${
        formatUnits(price, 6)
      } USD/XRP; ${STEPS_MAX} steps and not done: ${done.join(", ")}; ${who}`,
      ok: false,
    };
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    const steps = done.length === 0 ? "" : ` after ${done.join(", ")}`;
    return { line: `failed${steps}: ${message}; ${who}`, ok: false };
  } finally {
    await ledger.close();
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

const { values } = parseArgs({ options: { "dry-run": { type: "boolean" } } });
const watchdog = setTimeout(() => {
  console.log(`dsusd-keeper: no end after ${RUN_TIMEOUT_MS} ms`);
  process.exit(1);
}, RUN_TIMEOUT_MS);
try {
  const { line, ok } = await run(values["dry-run"] === true);
  console.log(`dsusd-keeper: ${line}`);
  process.exitCode = ok ? 0 : 1;
} catch (error) {
  console.log(
    `dsusd-keeper: failed: ${
      error instanceof Error ? error.message : String(error)
    }`,
  );
  process.exitCode = 1;
} finally {
  clearTimeout(watchdog);
}
// A run that failed early can leave a connection or a request open, which
// would keep Node running after its line until systemd stops it, and hold
// back the timer's next run.
process.exit();
