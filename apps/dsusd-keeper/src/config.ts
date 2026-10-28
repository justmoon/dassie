// The keeper's configuration, from its environment. On its VM the seeds
// come from Secret Manager (vm/run.sh); nothing else is secret.

import type { Config } from "./plan.ts";
import { DROPS_PER_XRP, type Result } from "./units.ts";

/** The XRP Ledger testnet's public server and faucet. */
export const TESTNET_SERVER = "wss://s.altnet.rippletest.net:51233";
export const TESTNET_FAUCET = "https://faucet.altnet.rippletest.net/accounts";

/** The pool's depth by default, in XRP, and the bounds of the setting.
 * Deep enough that buying 50 DSUSD moves the pool's price by less than half a
 * percent: owners and the market maker buy dollars in tens at a time, and
 * a pool of a thousand XRP moved several percent at each. The faucet
 * fills it over a few hours, a few requests at a time. */
export const DEPTH_XRP_DEFAULT = 20_000n;
const DEPTH_XRP_MIN = 10n;
const DEPTH_XRP_MAX = 1_000_000n;
/** The band around the target by default, in millionths, and its bound. */
export const BAND_PPM_DEFAULT = 2_000n;
const BAND_PPM_MAX = 100_000n;

export type Settings = Config & {
  issuerSeed: string;
  keeperSeed: string;
  server: string;
  faucet: string;
};

/** The settings from `env`: DSUSD_ISSUER_SEED and DSUSD_KEEPER_SEED, and
 * optionally DSUSD_KEEPER_SERVER, DSUSD_KEEPER_FAUCET,
 * DSUSD_KEEPER_DEPTH_XRP and DSUSD_KEEPER_BAND_PPM. */
export function settingsFrom(
  env: Record<string, string | undefined>,
): Result<Settings> {
  const issuerSeed = env["DSUSD_ISSUER_SEED"]?.trim() ?? "";
  const keeperSeed = env["DSUSD_KEEPER_SEED"]?.trim() ?? "";
  if (issuerSeed === "" || keeperSeed === "") {
    return {
      ok: false,
      error: "DSUSD_ISSUER_SEED and DSUSD_KEEPER_SEED are required",
    };
  }
  if (issuerSeed === keeperSeed) {
    return {
      ok: false,
      error: "the issuer and the keeper need seeds of their own",
    };
  }
  const server = env["DSUSD_KEEPER_SERVER"] ?? TESTNET_SERVER;
  if (!/^wss?:\/\//.test(server)) {
    return {
      ok: false,
      error: `DSUSD_KEEPER_SERVER: ${server} is no WebSocket URL`,
    };
  }
  const faucet = env["DSUSD_KEEPER_FAUCET"] ?? TESTNET_FAUCET;
  if (!/^https?:\/\//.test(faucet)) {
    return {
      ok: false,
      error: `DSUSD_KEEPER_FAUCET: ${faucet} is no HTTP URL`,
    };
  }
  const depth = integer(env["DSUSD_KEEPER_DEPTH_XRP"], DEPTH_XRP_DEFAULT);
  if (depth === null || depth < DEPTH_XRP_MIN || depth > DEPTH_XRP_MAX) {
    return {
      ok: false,
      error:
        `DSUSD_KEEPER_DEPTH_XRP: whole XRP from ${DEPTH_XRP_MIN} to ${DEPTH_XRP_MAX}`,
    };
  }
  const bandPpm = integer(env["DSUSD_KEEPER_BAND_PPM"], BAND_PPM_DEFAULT);
  if (bandPpm === null || bandPpm > BAND_PPM_MAX) {
    return { ok: false, error: `DSUSD_KEEPER_BAND_PPM: 0 to ${BAND_PPM_MAX}` };
  }
  return {
    ok: true,
    value: {
      issuerSeed,
      keeperSeed,
      server,
      faucet,
      depthDrops: depth * DROPS_PER_XRP,
      bandPpm,
    },
  };
}

function integer(text: string | undefined, fallback: bigint): bigint | null {
  if (text === undefined || text.trim() === "") return fallback;
  return /^\d{1,9}$/.test(text.trim()) ? BigInt(text.trim()) : null;
}
