// What the keeper reads from the ledger, parsed from the server's answers
// (the `result` of each request) into the state the plan works from.
// Anything malformed is an error value: the run stops rather than act on
// a state it misread.

import type { Pool } from "./amm.ts";
import { FEE_UNITS_MAX } from "./amm.ts";
import {
  type Decimal,
  DSUSD_CURRENCY,
  DSUSD_DECIMALS,
  floorUnits,
  parseDecimal,
  type Result,
} from "./units.ts";

/** An account the keeper uses, as `account_info` shows it. */
export type Account = { drops: bigint; defaultRipple: boolean };

/** The keeper's DSUSD line, from its side. */
export type Line = {
  balanceMicros: bigint;
  /** Whether the keeper's limit is at least `LINE_LIMIT_MIN_DSUSD`. */
  limitOk: boolean;
  noRipple: boolean;
};

/** The account root's DefaultRipple flag (lsfDefaultRipple). */
export const LSF_DEFAULT_RIPPLE = 0x0080_0000;
/** The least limit the keeper's line may have, in whole DSUSD: far above
 * anything the keeper holds, so no mint fails for the limit. */
export const LINE_LIMIT_MIN_DSUSD = 10n ** 15n;
/** The ledger's largest value, the limit the keeper sets. */
export const LINE_LIMIT = "9999999999999999e80";

type Fields = Record<string, unknown>;

function object(value: unknown, what: string): Result<Fields> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? { ok: true, value: value as Fields }
    : { ok: false, error: `${what}: not an object` };
}

function decimal(value: unknown, what: string): Result<Decimal> {
  if (typeof value !== "string") {
    return { ok: false, error: `${what}: not a string` };
  }
  const parsed = parseDecimal(value);
  return parsed.ok ? parsed : { ok: false, error: `${what}: ${parsed.error}` };
}

function drops(value: unknown, what: string): Result<bigint> {
  if (typeof value !== "string" || !/^\d{1,20}$/.test(value)) {
    return { ok: false, error: `${what}: not drops` };
  }
  return { ok: true, value: BigInt(value) };
}

/** An account from `account_info`'s result. */
export function parseAccount(result: unknown): Result<Account> {
  const fields = object(result, "account_info");
  if (!fields.ok) return fields;
  const data = object(fields.value["account_data"], "account_data");
  if (!data.ok) return data;
  const balance = drops(data.value["Balance"], "Balance");
  if (!balance.ok) return balance;
  const flags = data.value["Flags"];
  if (typeof flags !== "number" || !Number.isInteger(flags) || flags < 0) {
    return { ok: false, error: "account_data: no Flags" };
  }
  return {
    ok: true,
    value: {
      drops: balance.value,
      defaultRipple: (flags & LSF_DEFAULT_RIPPLE) !== 0,
    },
  };
}

/** The line of `currency` from `account_lines`' result, from the side of
 * the account asked for; null when there is none. */
function findLine(
  result: unknown,
  currency: string,
): Result<Fields | null> {
  const fields = object(result, "account_lines");
  if (!fields.ok) return fields;
  const lines = fields.value["lines"];
  if (!Array.isArray(lines)) {
    return { ok: false, error: "account_lines: no lines" };
  }
  for (const line of lines) {
    const entry = object(line, "line");
    if (!entry.ok) return entry;
    if (entry.value["currency"] === currency) return entry;
  }
  return { ok: true, value: null };
}

/** The keeper's DSUSD line from `account_lines`' result (asked with the
 * issuer as the peer); null when there is none. */
export function parseLine(result: unknown): Result<Line | null> {
  const line = findLine(result, DSUSD_CURRENCY);
  if (!line.ok) return line;
  if (line.value === null) return { ok: true, value: null };
  const balance = decimal(line.value["balance"], "balance");
  if (!balance.ok) return balance;
  const limit = decimal(line.value["limit"], "limit");
  if (!limit.ok) return limit;
  return {
    ok: true,
    value: {
      balanceMicros: floorUnits(balance.value, DSUSD_DECIMALS),
      limitOk: floorUnits(limit.value, 0) >= LINE_LIMIT_MIN_DSUSD,
      noRipple: line.value["no_ripple"] === true,
    },
  };
}

/** The keeper's LP tokens from `account_lines`' result (asked with the
 * pool's account as the peer): zero when it holds none. */
export function parseLpTokens(
  result: unknown,
  currency: string,
): Result<Decimal> {
  const line = findLine(result, currency);
  if (!line.ok) return line;
  if (line.value === null) {
    return { ok: true, value: { mantissa: 0n, exponent: 0 } };
  }
  const balance = decimal(line.value["balance"], "LP balance");
  if (!balance.ok) return balance;
  if (balance.value.mantissa < 0n) {
    return { ok: false, error: "LP balance: negative" };
  }
  return balance;
}

/** The pool from `amm_info`'s result, without the keeper's LP tokens
 * (zero), and the pool's account and LP token currency, which the keeper's
 * LP tokens are read with. The two amounts are told apart by kind, not by
 * order: XRP is a string of drops, DSUSD an issued amount. */
export function parsePool(
  result: unknown,
  issuer: string,
): Result<{ pool: Pool; account: string; lpCurrency: string }> {
  const fields = object(result, "amm_info");
  if (!fields.ok) return fields;
  const amm = object(fields.value["amm"], "amm");
  if (!amm.ok) return amm;
  let xrp: bigint | null = null;
  let dsusd: bigint | null = null;
  for (const key of ["amount", "amount2"]) {
    const amount = amm.value[key];
    if (typeof amount === "string") {
      const value = drops(amount, key);
      if (!value.ok) return value;
      xrp = value.value;
      continue;
    }
    const token = object(amount, key);
    if (!token.ok) return token;
    if (
      token.value["currency"] !== DSUSD_CURRENCY ||
      token.value["issuer"] !== issuer
    ) {
      return { ok: false, error: `${key}: not DSUSD of the issuer` };
    }
    const value = decimal(token.value["value"], key);
    if (!value.ok) return value;
    dsusd = floorUnits(value.value, DSUSD_DECIMALS);
  }
  if (xrp === null || dsusd === null || dsusd < 0n) {
    return { ok: false, error: "amm: not an XRP/DSUSD pool" };
  }
  const lp = object(amm.value["lp_token"], "lp_token");
  if (!lp.ok) return lp;
  const lpTotal = decimal(lp.value["value"], "lp_token");
  if (!lpTotal.ok) return lpTotal;
  if (lpTotal.value.mantissa < 0n) {
    return { ok: false, error: "lp_token: negative" };
  }
  const lpCurrency = lp.value["currency"];
  const account = amm.value["account"];
  if (typeof lpCurrency !== "string" || typeof account !== "string") {
    return { ok: false, error: "amm: no account or LP currency" };
  }
  const feeUnits = amm.value["trading_fee"];
  if (
    typeof feeUnits !== "number" || !Number.isInteger(feeUnits) ||
    feeUnits < 0 || feeUnits > FEE_UNITS_MAX
  ) {
    return { ok: false, error: "amm: no trading fee" };
  }
  return {
    ok: true,
    value: {
      pool: {
        drops: xrp,
        micros: dsusd,
        lpTotal: lpTotal.value,
        lpKeeper: { mantissa: 0n, exponent: 0 },
        feeUnits,
      },
      account,
      lpCurrency,
    },
  };
}
