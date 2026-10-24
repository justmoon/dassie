/**
 * End to end (spec/areas/testing.md#end-to-end): mainnet's schemes in the
 * live realm (spec/decisions/mainnet-schemes.md), on the fake ledger's
 * mainnet, never real money. The nodes are the real binary, built with
 * debug assertions, set to the live realm through the dev switch
 * (`DASSIE_DEV_LIVE_REALM`) before they are claimed; a release binary
 * refuses that switch, which the release checks test.
 *
 * Mainnet has no faucet, so each node's account is funded by a deposit, as
 * its owner would fund it. In XRP (`xrpl-live-settlement`) and in RLUSD
 * (`rlusd-live-settlement`), a client of d1 pays d2's receiver several
 * times the credit, so d1 settles on the ledger as it pays, at addresses
 * under `g.`. Then RLUSD's pool is moved off its peg: once the guard's hold
 * has passed, which dev timers make seconds, each node pauses its RLUSD
 * scheme and says why, still books what arrives on its line, and runs it
 * again once the pool is back at a dollar for the recovery hold
 * (spec/areas/xrpl.md#the-peg-guard).
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { LedgerConnection, PegPause, Realm } from "@dassie/rpc/proto";

import {
  DASSIE,
  FAKE_LEDGER,
  missingScenarioTools,
  SCENARIO_PORT_OFFSET,
} from "./harness.ts";
import { CREDIT, ledger, pay, peerAccounts, settled } from "./settling.ts";
import {
  DEV_RATES,
  poolDrops,
  Scenario,
  type ScenarioNode,
} from "@dassie/dev/scenario";

const XRP_LEDGER = "xrpl+xrp";
const RLUSD_LEDGER = "xrpl-rlusd+usd";
/** RLUSD's issuer on mainnet, which the fake ledger's mainnet issues it at. */
const ISSUER = "rMxCKbEDwqr76QuheSUMdEGf4B9xJ8m5De";
/** 5 XRP or 5 dollars: over four times what a peer extends, so d1 must
 * settle. */
const PAYMENT = 5_000_000_000n;
/** What the scenario's issuer mints each node: 100 dollars. */
const MINTED = 100_000_000_000n;
/** A dollar, in the ledger's internal units. */
const DOLLAR = 1_000_000_000n;
/** The ledger's unit, a millionth of a dollar, in internal units. */
const UNIT = 1_000n;
/** The scenario's pool: as many dollars of RLUSD, and XRP worth as much at
 * the dev rate, 200,000 XRP, twice RLUSD's floor. */
const POOL_VALUE = "100000";
const POOL_FEE = 500;
/**
 * The two tests run one after the other, on the same ports, clear of the
 * other files' (harness.ts).
 */
const PORT_OFFSET = SCENARIO_PORT_OFFSET + 900;

const missing = await missingScenarioTools();

/** Starts scenario `id`, in a directory of its own. */
function start(id: string): Promise<Scenario> {
  return Scenario.temporary(id, {
    dassie: DASSIE,
    fakeLedger: FAKE_LEDGER,
    portOffset: PORT_OFFSET,
  });
}

/**
 * Every node runs in the live realm: its ledgers are of it, the schemes it
 * offers are mainnet's alone, and an incoming payment's address is under
 * `g.`.
 */
async function assertLive(node: ScenarioNode, id: string): Promise<void> {
  assertEquals((await ledger(node, id)).realm, Realm.LIVE);
  const { schemes } = await node.rpc.call("listSettlementSchemes", {});
  assert(schemes.length > 0, `${node.name} offers no scheme`);
  for (const scheme of schemes) {
    assertEquals(scheme.realm, Realm.LIVE, `${node.name} offers ${scheme.id}`);
  }
  assertEquals(
    schemes.map((scheme) => scheme.id).sort(),
    ["xrpl", "xrpl-rlusd"],
  );
  const { payment } = await node.rpc.call("createIncomingPayment", {});
  assert(
    payment?.destinationAccount.startsWith("g."),
    payment?.destinationAccount,
  );
}

test(
  "xrpl-live-settlement settles in XRP on the fake mainnet, in the live realm",
  { skip: missing !== undefined },
  async () => {
    const scenario = await start("xrpl-live-settlement");
    try {
      const funded = async () => {
        for (const node of scenario.nodes) {
          const view = await ledger(node, XRP_LEDGER);
          if (view.connection !== LedgerConnection.SYNCED) return false;
          if (view.ownerBalance <= 0n) return false;
        }
        return true;
      };
      await scenario.until("funded by a deposit", funded, 120_000);
      await scenario.until("peered", () => scenario.converged(), 120_000);
      const [d1, d2] = scenario.nodes;
      for (const node of scenario.nodes) {
        await assertLive(node, XRP_LEDGER);
        // Mainnet has no faucet.
        assertEquals((await ledger(node, XRP_LEDGER)).faucet, false);
        const { externalAssets } = await node.rpc.call("listLedgers", {});
        assertEquals(externalAssets.map((asset) => asset.id), [
          "xrpl-account/0",
        ]);
      }
      await scenario.until(
        "paid in advance",
        () => settled(scenario, XRP_LEDGER),
        120_000,
      );

      const before = await ledger(d1, XRP_LEDGER);
      const address = await pay(scenario, [d1, d2], XRP_LEDGER, PAYMENT);
      assert(address.startsWith("g."), address);
      await scenario.until(
        "settled",
        () => settled(scenario, XRP_LEDGER),
        120_000,
      );
      // d1 paid in XRP on the ledger, and owes d2 no more than the credit
      // a junior extends.
      const after = await ledger(d1, XRP_LEDGER);
      assert(
        after.onLedgerBalance < before.onLedgerBalance,
        "d1 settled on the ledger",
      );
      const owes = (await peerAccounts(scenario, XRP_LEDGER)).get("d1>d2")!;
      assert(owes <= CREDIT, `d1 still owes ${owes}`);
      assert(after.ownerBalance <= before.ownerBalance - PAYMENT);
    } finally {
      const failures = await scenario.stop();
      assertEquals(failures, [], failures.join("\n"));
    }
  },
);

/** What a node's account holds of RLUSD, as the ledger says, in the
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

/** Whether a node's books hold its RLUSD line. */
async function inStep(node: ScenarioNode): Promise<boolean> {
  const view = await ledger(node, RLUSD_LEDGER);
  return view.onLedgerBalance === (await line(node)).units;
}

/** What every node's peg guard says. */
async function guards(scenario: Scenario) {
  return await Promise.all(
    scenario.nodes.map(async (node) => (await ledger(node, RLUSD_LEDGER)).peg),
  );
}

test(
  "rlusd-live-settlement settles in RLUSD on the fake mainnet, and pauses while RLUSD is off its peg",
  { skip: missing !== undefined },
  async () => {
    const scenario = await start("rlusd-live-settlement");
    try {
      const funded = async () => {
        for (const node of scenario.nodes) {
          const view = await ledger(node, RLUSD_LEDGER);
          if (view.connection !== LedgerConnection.SYNCED) return false;
          if (!(await inStep(node)) || view.ownerBalance < MINTED) {
            return false;
          }
        }
        return true;
      };
      await scenario.until("funded and minted", funded, 120_000);
      await scenario.until("peered", () => scenario.converged(), 120_000);
      const [d1, d2] = scenario.nodes;
      for (const node of scenario.nodes) {
        await assertLive(node, RLUSD_LEDGER);
        // The line is to RLUSD's mainnet issuer, and the guard reads the
        // pool at a dollar.
        assertEquals((await line(node)).holding.issuer, ISSUER);
        assertEquals((await ledger(node, RLUSD_LEDGER)).issuer, ISSUER);
      }
      const atPar = async () =>
        (await guards(scenario)).every((peg) =>
          peg?.paused === PegPause.UNSPECIFIED && peg.pricePpm === 1_000_000n
        );
      await scenario.until("read at a dollar", atPar, 60_000);
      await scenario.until(
        "paid in advance",
        () => settled(scenario, RLUSD_LEDGER, inStep),
        120_000,
      );

      const before = await ledger(d1, RLUSD_LEDGER);
      const address = await pay(scenario, [d1, d2], RLUSD_LEDGER, PAYMENT);
      assert(address.startsWith("g."), address);
      await scenario.until(
        "settled",
        () => settled(scenario, RLUSD_LEDGER, inStep),
        120_000,
      );
      const after = await ledger(d1, RLUSD_LEDGER);
      assert(
        after.onLedgerBalance < before.onLedgerBalance,
        "d1 settled in RLUSD on the ledger",
      );
      const owes = (await peerAccounts(scenario, RLUSD_LEDGER)).get("d1>d2")!;
      assert(owes <= CREDIT, `d1 still owes ${owes}`);

      // The pool prices RLUSD 5% under a dollar: once every reading has
      // been off for the hold, each node pauses the scheme and says why.
      const par = poolDrops(POOL_VALUE, DEV_RATES.XRP);
      const setPool = (drops: bigint) =>
        scenario.askIssuer({
          action: "amm",
          token: "RLUSD",
          drops: `${drops}`,
          value: POOL_VALUE,
          fee: POOL_FEE,
        });
      await setPool(par * 95n / 100n);
      const offPeg = async () =>
        (await guards(scenario)).every((peg) =>
          peg?.paused === PegPause.OFF_PEG && peg.pricePpm === 950_000n
        );
      await scenario.until("paused off its peg", offPeg, 60_000);

      // What arrives on the line is still booked while paused.
      const owned = (await ledger(d2, RLUSD_LEDGER)).ownerBalance;
      const { address: d2Account } = (await d2.rpc.call("listLedgers", {}))
        .externalAssets[0];
      await scenario.askIssuer({
        token: "RLUSD",
        action: "mint",
        destination: d2Account,
        value: "1",
      });
      await scenario.until(
        "the deposit booked while paused",
        async () =>
          (await ledger(d2, RLUSD_LEDGER)).ownerBalance === owned + DOLLAR &&
          (await inStep(d2)),
        60_000,
      );
      assert(await offPeg(), "still paused");

      // Back at a dollar for the recovery hold, it runs again.
      await setPool(par);
      await scenario.until("running again at a dollar", atPar, 180_000);

      // Drained below RLUSD's floor of XRP, the pool is no price: once no
      // reading has counted for the staleness bound, the guard fails
      // closed. The first reading at a dollar again ends that.
      await setPool(par / 4n);
      const stale = async () =>
        (await guards(scenario)).every((peg) => peg?.paused === PegPause.STALE);
      await scenario.until("paused, its price unread", stale, 60_000);
      await setPool(par);
      await scenario.until("running again once read", atPar, 60_000);
    } finally {
      const failures = await scenario.stop();
      assertEquals(failures, [], failures.join("\n"));
    }
  },
);
