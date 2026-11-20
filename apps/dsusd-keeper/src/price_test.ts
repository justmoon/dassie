import { readFileSync } from "node:fs";
import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import {
  LEDGER_AGE_MS_MAX,
  LEDGER_AHEAD_MS_MAX,
  POOL_DROPS_MIN,
  RLUSD_CURRENCY,
  RLUSD_ISSUER,
  targetPrice,
} from "./price.ts";

/** Mainnet's answers, as the keeper read them from its default server
 * (testdata/capture-mainnet-pool.ts). */
const CAPTURED = JSON.parse(
  readFileSync(
    new URL("../testdata/mainnet-pool.json", import.meta.url),
    "utf8",
  ),
) as { ledger: Record<string, unknown>; amm: Record<string, unknown> };

/** When the captured ledger closed, by its close time, in milliseconds
 * since 1970. */
const CLOSED_MS = 1_795_181_452_000;

/** The captured answers, with the pool's fields changed by `pool`, the
 * ledger header's by `header`, and the answers' own by `ledger` and
 * `amm`. */
function answers(
  changes: {
    pool?: Record<string, unknown>;
    ledger?: Record<string, unknown>;
    header?: Record<string, unknown>;
    amm?: Record<string, unknown>;
  } = {},
): { ledger: unknown; amm: unknown } {
  const ledger = structuredClone(CAPTURED.ledger);
  const amm = structuredClone(CAPTURED.amm);
  return {
    ledger: {
      ...ledger,
      ledger: { ...(ledger["ledger"] as object), ...changes.header },
      ...changes.ledger,
    },
    amm: {
      ...amm,
      amm: { ...(amm["amm"] as object), ...changes.pool },
      ...changes.amm,
    },
  };
}

function priced(
  changes: Parameters<typeof answers>[0] = {},
  nowMs = CLOSED_MS + 4_000,
) {
  const { ledger, amm } = answers(changes);
  return targetPrice(ledger, amm, nowMs);
}

test("the target is the pool's RLUSD per XRP, in millionths, RLUSD at par", () => {
  const target = priced();
  assert(target.ok);
  // 1,899,847.950220252 RLUSD over 2,023,664.867701 XRP, rounded down.
  assertEquals(target.value.usdMicrosPerXrp, 938_815n);
  assertEquals(target.value.ledgerIndex, 108_454_740);
  assertEquals(target.value.drops, 2_023_664_867_701n);
});

test("the two amounts are told apart by kind, not by order", () => {
  const pool = CAPTURED.amm["amm"] as Record<string, unknown>;
  const target = priced({
    pool: { amount: pool["amount2"], amount2: pool["amount"] },
  });
  assert(target.ok);
  assertEquals(target.value.usdMicrosPerXrp, 938_815n);
});

test("a ledger that closed too long ago, or ahead of our clock, is refused", () => {
  assert(priced({}, CLOSED_MS + LEDGER_AGE_MS_MAX).ok);
  assertEquals(priced({}, CLOSED_MS + LEDGER_AGE_MS_MAX + 1).ok, false);
  assert(priced({}, CLOSED_MS - LEDGER_AHEAD_MS_MAX).ok);
  assertEquals(priced({}, CLOSED_MS - LEDGER_AHEAD_MS_MAX - 1).ok, false);
});

test("a pool with less XRP than the floor is refused", () => {
  // The floor, at the captured ledger's price.
  const rlusd = (POOL_DROPS_MIN * 938_815n / 1_000_000n).toString();
  const value = `${rlusd.slice(0, -6)}.${rlusd.slice(-6)}`;
  const at = (drops: bigint) =>
    priced({
      pool: {
        amount: drops.toString(),
        amount2: { currency: RLUSD_CURRENCY, issuer: RLUSD_ISSUER, value },
      },
    });
  assert(at(POOL_DROPS_MIN).ok);
  assertEquals(at(POOL_DROPS_MIN - 1n).ok, false);
  assertEquals(at(0n).ok, false);
});

test("a pool not of mainnet's RLUSD, or not at the ledger, is refused", () => {
  const amount2 = (fields: Record<string, unknown>) => ({
    pool: {
      amount2: {
        currency: RLUSD_CURRENCY,
        issuer: RLUSD_ISSUER,
        value: "1899847.950220252",
        ...fields,
      },
    },
  });
  const refused: Parameters<typeof answers>[0][] = [
    // Another issuer's RLUSD: the testnet's.
    amount2({ issuer: "rQhWct2fv4Vc4KRjRgMrxa8xPN9Zx9iLKV" }),
    amount2({ currency: "USD" }),
    amount2({ value: "0" }),
    amount2({ value: "-5" }),
    amount2({ value: 1899847.95 }),
    amount2({ value: "abc" }),
    // Two XRP amounts, and two tokens.
    { pool: { amount2: "2023664867701" } },
    {
      pool: {
        amount: { currency: RLUSD_CURRENCY, issuer: RLUSD_ISSUER, value: "1" },
      },
    },
    { pool: { amount: "1.5" } },
    { pool: { amount: undefined } },
    { pool: { asset2_frozen: true } },
    { pool: { asset_frozen: true } },
    // Not validated, or not the ledger whose close time was read.
    { ledger: { validated: false } },
    { amm: { validated: false } },
    { amm: { ledger_hash: "00".repeat(32) } },
    { header: { ledger_hash: "00".repeat(32) } },
    { header: { close_time: "848496652" } },
    { ledger: { ledger_index: "108454740" } },
    { ledger: { ledger: null } },
    { amm: { amm: [] } },
    // Out of the prices the keeper believes.
    amount2({ value: "1000" }),
    amount2({ value: "1e10" }),
  ];
  for (const changes of refused) {
    assertEquals(priced(changes).ok, false, JSON.stringify(changes));
  }
  assertEquals(targetPrice(null, CAPTURED.amm, CLOSED_MS).ok, false);
  assertEquals(targetPrice(CAPTURED.ledger, "pool", CLOSED_MS).ok, false);
});
