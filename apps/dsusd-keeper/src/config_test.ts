import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import {
  BAND_PPM_DEFAULT,
  DEPTH_XRP_DEFAULT,
  settingsFrom,
  TESTNET_FAUCET,
  TESTNET_SERVER,
} from "./config.ts";

const SEEDS = { DSUSD_ISSUER_SEED: "sIssuer", DSUSD_KEEPER_SEED: "sKeeper" };

test("the seeds are all a run needs; the rest has defaults", () => {
  const settings = settingsFrom(SEEDS);
  assert(settings.ok);
  assertEquals(settings.value, {
    issuerSeed: "sIssuer",
    keeperSeed: "sKeeper",
    server: TESTNET_SERVER,
    faucet: TESTNET_FAUCET,
    depthDrops: DEPTH_XRP_DEFAULT * 1_000_000n,
    bandPpm: BAND_PPM_DEFAULT,
  });
});

test("the settings can be given", () => {
  const settings = settingsFrom({
    ...SEEDS,
    DSUSD_KEEPER_SERVER: "ws://localhost:6006",
    DSUSD_KEEPER_FAUCET: "http://localhost:8080/accounts",
    DSUSD_KEEPER_DEPTH_XRP: "250",
    DSUSD_KEEPER_BAND_PPM: "0",
  });
  assert(settings.ok);
  assertEquals(settings.value.depthDrops, 250_000_000n);
  assertEquals(settings.value.bandPpm, 0n);
  assertEquals(settings.value.server, "ws://localhost:6006");
});

test("missing or shared seeds and settings out of bounds are refused", () => {
  const refused: Record<string, string | undefined>[] = [
    {},
    { DSUSD_ISSUER_SEED: "sIssuer" },
    { DSUSD_ISSUER_SEED: "sSame", DSUSD_KEEPER_SEED: "sSame" },
    { ...SEEDS, DSUSD_KEEPER_SERVER: "https://example.com" },
    { ...SEEDS, DSUSD_KEEPER_FAUCET: "ftp://example.com" },
    { ...SEEDS, DSUSD_KEEPER_DEPTH_XRP: "5" },
    { ...SEEDS, DSUSD_KEEPER_DEPTH_XRP: "1.5" },
    { ...SEEDS, DSUSD_KEEPER_DEPTH_XRP: "2000000" },
    { ...SEEDS, DSUSD_KEEPER_BAND_PPM: "-1" },
    { ...SEEDS, DSUSD_KEEPER_BAND_PPM: "200000" },
  ];
  for (const env of refused) {
    assertEquals(settingsFrom(env).ok, false, JSON.stringify(env));
  }
});
