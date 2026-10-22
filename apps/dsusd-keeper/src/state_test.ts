import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import {
  LINE_LIMIT,
  parseAccount,
  parseLine,
  parseLpTokens,
  parsePool,
} from "./state.ts";
import { DSUSD_CURRENCY } from "./units.ts";

const ISSUER = "rDSUSDissuerXXXXXXXXXXXXXXXXXXXXX";
const AMM = "rAMMaccountXXXXXXXXXXXXXXXXXXXXXX";
const LP = "03A1B2C3D4E5F60718293A4B5C6D7E8F90A1B2C3";

test("an account's balance and DefaultRipple", () => {
  const account = parseAccount({
    account_data: { Account: ISSUER, Balance: "100000000", Flags: 0x0080_0000 },
    ledger_index: 5,
    validated: true,
  });
  assertEquals(account, {
    ok: true,
    value: { drops: 100_000_000n, defaultRipple: true },
  });
  const plain = parseAccount({ account_data: { Balance: "1", Flags: 0 } });
  assertEquals(plain, { ok: true, value: { drops: 1n, defaultRipple: false } });
  for (
    const bad of [null, {}, { account_data: { Balance: "1.5", Flags: 0 } }, {
      account_data: { Balance: "1" },
    }]
  ) {
    assertEquals(parseAccount(bad).ok, false);
  }
});

test("the keeper's DSUSD line, from its side", () => {
  const lines = {
    account: "rKeeper",
    lines: [
      { account: ISSUER, currency: "USD", balance: "5", limit: "10" },
      {
        account: ISSUER,
        currency: DSUSD_CURRENCY,
        balance: "12.3456789",
        limit: LINE_LIMIT,
        limit_peer: "0",
        no_ripple: true,
      },
    ],
  };
  assertEquals(parseLine(lines), {
    ok: true,
    value: { balanceMicros: 12_345_678n, limitOk: true, noRipple: true },
  });
  const low = {
    lines: [{ currency: DSUSD_CURRENCY, balance: "0", limit: "1000" }],
  };
  assertEquals(parseLine(low), {
    ok: true,
    value: { balanceMicros: 0n, limitOk: false, noRipple: false },
  });
  assertEquals(parseLine({ lines: [] }), { ok: true, value: null });
  assertEquals(
    parseLine({ lines: [{ currency: DSUSD_CURRENCY, balance: "x" }] }).ok,
    false,
  );
  assertEquals(parseLine({}).ok, false);
});

test("the keeper's LP tokens, zero without a line", () => {
  const lines = {
    lines: [{ account: AMM, currency: LP, balance: "1581138.830084189" }],
  };
  assertEquals(parseLpTokens(lines, LP), {
    ok: true,
    value: { mantissa: 1581138830084189n, exponent: -9 },
  });
  assertEquals(parseLpTokens({ lines: [] }, LP), {
    ok: true,
    value: { mantissa: 0n, exponent: 0 },
  });
  assertEquals(
    parseLpTokens({ lines: [{ currency: LP, balance: "-1" }] }, LP).ok,
    false,
  );
});

/** `amm_info`'s result for an XRP/DSUSD pool. */
function ammInfo(amm: Record<string, unknown> = {}): unknown {
  return {
    amm: {
      account: AMM,
      amount: "1000000000",
      amount2: {
        currency: DSUSD_CURRENCY,
        issuer: ISSUER,
        value: "2500.0000001",
      },
      lp_token: { currency: LP, issuer: AMM, value: "1581138.830084189" },
      trading_fee: 300,
      ...amm,
    },
    ledger_index: 7,
    validated: true,
  };
}

test("the pool, its sides told apart by kind", () => {
  const parsed = parsePool(ammInfo(), ISSUER);
  assert(parsed.ok);
  assertEquals(parsed.value.account, AMM);
  assertEquals(parsed.value.lpCurrency, LP);
  assertEquals(parsed.value.pool.drops, 1_000_000_000n);
  assertEquals(parsed.value.pool.micros, 2_500_000_000n);
  assertEquals(parsed.value.pool.feeUnits, 300);
  const swapped = parsePool(
    ammInfo({
      amount: { currency: DSUSD_CURRENCY, issuer: ISSUER, value: "2500" },
      amount2: "1000000000",
    }),
    ISSUER,
  );
  assert(swapped.ok);
  assertEquals(swapped.value.pool.drops, 1_000_000_000n);
  assertEquals(swapped.value.pool.micros, 2_500_000_000n);
});

test("a pool of something else, or malformed, is refused", () => {
  const broken = [
    ammInfo({
      amount2: { currency: DSUSD_CURRENCY, issuer: "rOther", value: "1" },
    }),
    ammInfo({ amount2: { currency: "USD", issuer: ISSUER, value: "1" } }),
    ammInfo({ amount2: "5" }),
    ammInfo({ trading_fee: 5000 }),
    ammInfo({ lp_token: { currency: LP, issuer: AMM, value: "-1" } }),
    ammInfo({ account: undefined }),
    {},
  ];
  for (const answer of broken) {
    assertEquals(parsePool(answer, ISSUER).ok, false, JSON.stringify(answer));
  }
});
