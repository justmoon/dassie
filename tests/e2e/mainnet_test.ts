/**
 * End to end (spec/areas/testing.md#end-to-end): mainnet's schemes in the
 * live realm (spec/decisions/mainnet-schemes.md) as owner books, which
 * peer with nobody (spec/areas/xrpl.md#owner-books), on the fake ledger's
 * mainnet, never real money. The node is the real binary, built with debug
 * assertions, set to the live realm through the dev switch
 * (`DASSIE_DEV_LIVE_REALM`) before it is claimed; a release binary refuses
 * that switch, which the release checks test.
 *
 * Mainnet has no faucet, so on `live-owner-books` the node's account is
 * funded by a deposit, as its owner would fund it, and RLUSD's issuer
 * mints it RLUSD. The XRP book books another deposit once, and still once
 * after the node restarts. Then RLUSD's pool is drained below its floor of
 * XRP, which is no price: once no reading has counted for the staleness
 * bound, the node pauses its RLUSD book's conversions, and the first
 * reading at a dollar runs them again. Then the pool is moved off its peg:
 * once the guard's hold has passed, which dev timers make seconds, the
 * node pauses them and says why, and still books what arrives on its line
 * (spec/areas/xrpl.md#the-peg-guard). Running again after the recovery
 * hold, a minute on dev timers, is the guard's unit tests' and the
 * engine's (`crates/dassie-node/src/engine/core/tests.rs`).
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { LedgerConnection, PegPause } from "@dassie/rpc/proto";

import {
  DASSIE,
  FAKE_LEDGER,
  missingScenarioTools,
  PORT_OFFSETS,
} from "./harness.ts";
import { assertLive, ledger } from "./settling.ts";
import {
  DEV_RATES,
  MAINNET_DEPOSIT_DROPS,
  poolDrops,
  Scenario,
  type ScenarioNode,
} from "@dassie/dev/scenario";

const XRP_LEDGER = "xrpl-xrp+xrp";
const RLUSD_LEDGER = "xrpl-rlusd+usd";
/** RLUSD's issuer on mainnet, which the fake ledger's mainnet issues it at. */
const ISSUER = "rMxCKbEDwqr76QuheSUMdEGf4B9xJ8m5De";
/** What the scenario's issuer mints the node: 100 dollars. */
const MINTED = 100_000_000_000n;
/** A dollar, in the ledger's internal units. */
const DOLLAR = 1_000_000_000n;
/** The ledger's unit, a millionth of a dollar, in internal units. */
const UNIT = 1_000n;
/** Internal units in a drop: XRP's ledgers are kept at scale 9, a drop at
 * scale 6. */
const UNITS_PER_DROP = 1_000n;
/** The fake ledger's base reserve, which the account's first deposit
 * leaves on the ledger: 1 XRP, in drops. */
const BASE_RESERVE_DROPS = 1_000_000n;
/** The deposit the XRP book is to book once: 5 XRP, in drops. */
const DEPOSIT_DROPS = 5_000_000n;
/** The scenario's pool: as many dollars of RLUSD, and XRP worth as much at
 * the dev rate, 200,000 XRP, twice RLUSD's floor. */
const POOL_VALUE = "100000";
const POOL_FEE = 500;
/** Clear of the other files' ports (harness.ts). */
const PORT_OFFSET = PORT_OFFSETS["mainnet_test.ts"];

const missing = await missingScenarioTools();

/** What the node's account holds of RLUSD, as the ledger says, in the
 * ledger's internal units: whole millionths, rounded down. */
async function line(node: ScenarioNode) {
  const { externalAssets } = await node.rpc.call("listLedgers", {});
  const holding = externalAssets
    .flatMap((asset) => asset.holdings)
    .find((holding) => holding.assetCode === "RLUSD");
  assert(holding, `${node.name} shows no RLUSD line`);
  const scale = BigInt(holding.assetScale);
  const micros = holding.amount / 10n ** (scale - 6n);
  return { holding, units: micros * UNIT };
}

/** Whether the node's RLUSD book holds its line. */
async function inStep(node: ScenarioNode): Promise<boolean> {
  const view = await ledger(node, RLUSD_LEDGER);
  return view.onLedgerBalance === (await line(node)).units;
}

/** What the node's two books hold, on the ledger and the owner's, with
 * nothing on its way. */
async function books(node: ScenarioNode) {
  return await Promise.all([XRP_LEDGER, RLUSD_LEDGER].map(async (id) => {
    const view = await ledger(node, id);
    return [view.onLedgerBalance, view.ownerBalance, view.ownerPending];
  }));
}

/** The deposits the node booked on the XRP book. */
async function deposits(node: ScenarioNode): Promise<bigint[]> {
  const { transfers } = await node.rpc.call("listTransfers", { limit: 1000 });
  return transfers
    .filter((transfer) =>
      transfer.reference.case === "other" &&
      transfer.reference.value === "deposit" &&
      transfer.creditPath.startsWith(`${XRP_LEDGER}:`)
    )
    .map((transfer) => transfer.amount)
    .sort((a, b) => a < b ? -1 : a > b ? 1 : 0);
}

test(
  "live-owner-books books XRP and RLUSD on the fake mainnet in owner books, and pauses RLUSD while it is off its peg",
  { skip: missing !== undefined },
  async () => {
    const scenario = await Scenario.temporary("live-owner-books", {
      dassie: DASSIE,
      fakeLedger: FAKE_LEDGER,
      portOffset: PORT_OFFSET,
    });
    try {
      const [node] = scenario.nodes;
      const funded = async () => {
        for (const id of [XRP_LEDGER, RLUSD_LEDGER]) {
          const view = await ledger(node, id);
          if (view.connection !== LedgerConnection.SYNCED) return false;
        }
        const rlusd = await ledger(node, RLUSD_LEDGER);
        return (await ledger(node, XRP_LEDGER)).ownerBalance > 0n &&
          (await inStep(node)) && rlusd.ownerBalance === MINTED;
      };
      await scenario.until("funded and minted", funded, 120_000);
      await assertLive(node, RLUSD_LEDGER);
      await assertLive(node, XRP_LEDGER);
      // Mainnet has no faucet; both books share the node's one account.
      assertEquals((await ledger(node, XRP_LEDGER)).faucet, false);
      const { externalAssets } = await node.rpc.call("listLedgers", {});
      assertEquals(externalAssets.map((asset) => asset.id), [
        "xrpl-account/0",
      ]);
      // The line is to RLUSD's mainnet issuer.
      assertEquals((await line(node)).holding.issuer, ISSUER);
      assertEquals((await ledger(node, RLUSD_LEDGER)).issuer, ISSUER);
      // Owner books peer with nobody.
      assertEquals((await node.rpc.call("listPeers", {})).peers, []);
      // The scenario's deposit, booked once: less the base reserve the
      // ledger holds back.
      const first = await deposits(node);
      assertEquals(
        first,
        [(MAINNET_DEPOSIT_DROPS - BASE_RESERVE_DROPS) * UNITS_PER_DROP],
        "the scenario's deposit",
      );

      // Another deposit, from outside: the XRP book takes it once, on the
      // ledger and as the owner's, and still once after a restart.
      const [[onLedger, owned]] = await books(node);
      await scenario.deposit(externalAssets[0].address, DEPOSIT_DROPS);
      const deposited = DEPOSIT_DROPS * UNITS_PER_DROP;
      await scenario.until("the deposit booked", async () => {
        const [[nowOnLedger, nowOwned, pending]] = await books(node);
        return nowOnLedger === onLedger + deposited &&
          nowOwned === owned + deposited && pending === 0n;
      }, 60_000);
      const kept = await books(node);
      assertEquals(kept[0], [onLedger + deposited, owned + deposited, 0n]);
      assertEquals(
        await deposits(node),
        [deposited, first[0]],
        "the deposit booked once",
      );
      // Once synced again after the restart, the node has read the
      // account's history again from where its book stood, the deposit's
      // ledger among it, and matched the balance as of a later ledger: a
      // deposit booked twice, by either, shows by then.
      const { code, output } = (await node.stop())!;
      assertEquals(code, 0, output);
      await node.start();
      await node.waitForEngine();
      await scenario.until(
        "synced again",
        async () =>
          (await ledger(node, XRP_LEDGER)).connection ===
            LedgerConnection.SYNCED,
        60_000,
      );
      assertEquals(await books(node), kept, "the books after a restart");
      assertEquals(
        await deposits(node),
        [deposited, first[0]],
        "each deposit booked once",
      );

      const guard = async () => (await ledger(node, RLUSD_LEDGER)).peg;
      const atPar = async () => {
        const peg = await guard();
        return peg?.paused === PegPause.UNSPECIFIED &&
          peg.pricePpm === 1_000_000n;
      };
      await scenario.until("read at a dollar", atPar, 60_000);

      const par = poolDrops(POOL_VALUE, DEV_RATES.XRP);
      const setPool = (drops: bigint) =>
        scenario.askIssuer({
          action: "amm",
          token: "RLUSD",
          drops: `${drops}`,
          value: POOL_VALUE,
          fee: POOL_FEE,
        });

      // Drained below RLUSD's floor of XRP, the pool is no price: once no
      // reading has counted for the staleness bound, the guard fails
      // closed. The first reading at a dollar again ends that.
      await setPool(par / 4n);
      const stale = async () => (await guard())?.paused === PegPause.STALE;
      await scenario.until("paused, its price unread", stale, 60_000);
      await setPool(par);
      await scenario.until("running again once read", atPar, 60_000);

      // The pool prices RLUSD 5% under a dollar: once every reading has
      // been off for the hold, the node pauses the book and says why.
      await setPool(par * 95n / 100n);
      const offPeg = async () => {
        const peg = await guard();
        return peg?.paused === PegPause.OFF_PEG && peg.pricePpm === 950_000n;
      };
      await scenario.until("paused off its peg", offPeg, 60_000);

      // What arrives on the line is still booked while paused.
      const before = (await ledger(node, RLUSD_LEDGER)).ownerBalance;
      await scenario.askIssuer({
        token: "RLUSD",
        action: "mint",
        destination: externalAssets[0].address,
        value: "1",
      });
      await scenario.until(
        "the deposit booked while paused",
        async () =>
          (await ledger(node, RLUSD_LEDGER)).ownerBalance ===
            before + DOLLAR && (await inStep(node)),
        60_000,
      );
      assert(await offPeg(), "still paused");
    } finally {
      const failures = await scenario.stop();
      assertEquals(failures, [], failures.join("\n"));
    }
  },
);
