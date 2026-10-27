/**
 * End to end (spec/areas/testing.md#end-to-end): the `dsusd-owner-book`
 * scenario holds DSUSD (spec/areas/xrpl.md#dsusd), the testnet's stand-in
 * for RLUSD, whose test token's scheme is disabled
 * (spec/decisions/issued-assets.md), in its node's owner book, which peers
 * with nobody (spec/areas/xrpl.md#owner-books). The node is the real
 * binary; the XRP Ledger is the node's fake, over WebSocket, with its
 * faucet and the issuer of its test DSUSD over HTTPS: only the fake mints
 * DSUSD on demand, and only it lets a test freeze, claw back and move a
 * pool.
 *
 * The faucet gives the node XRP for gas, it sets its trust line to DSUSD's
 * issuer, never RLUSD's, and the issuer mints it DSUSD, which its book
 * takes as a deposit. Then the pool is moved off its peg: once the guard's
 * hold has passed, which dev timers make seconds, the node pauses the
 * book's conversions and says why, and still books what arrives on its
 * line (spec/areas/xrpl.md#the-peg-guard). Then the issuer freezes the
 * line, which the owner sees, and claws back some of the DSUSD, which the
 * book takes as a withdrawal: the books stay in step with the line.
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { LedgerConnection, PegPause } from "@dassie/rpc/proto";

import {
  DASSIE,
  FAKE_LEDGER,
  missingScenarioTools,
  SCENARIO_PORT_OFFSET,
} from "./harness.ts";
import {
  DEV_RATES,
  poolDrops,
  Scenario,
  type ScenarioNode,
} from "@dassie/dev/scenario";

const LEDGER = "xrpl-testnet-dsusd+usd";
/** DSUSD's issuer on the testnet, which the fake ledger issues it at. */
const ISSUER = "rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M";
/** What the scenario's issuer mints the node: 100 dollars. */
const MINTED = 100_000_000_000n;
/** A dollar, in the ledger's internal units. */
const DOLLAR = 1_000_000_000n;
/** What the issuer claws back: 2 dollars. */
const CLAWED = 2_000_000_000n;
/** The ledger's unit, a millionth of a dollar, in internal units. */
const UNIT = 1_000n;
/** The scenario's pool: as many dollars of DSUSD, and XRP worth as much at
 * the dev rate. */
const POOL_VALUE = "50000";
const POOL_FEE = 500;
const PORT_OFFSET = SCENARIO_PORT_OFFSET + 200;

const missing = await missingScenarioTools();

/** The node's DSUSD ledger, as its owner sees it. */
async function ledger(node: ScenarioNode) {
  const { ledgers } = await node.rpc.call("listLedgers", {});
  const found = ledgers.find((l) => l.id === LEDGER);
  assert(found, `${node.name} has no ${LEDGER}`);
  return found;
}

/** What the node's account holds of DSUSD, as the ledger says, in the
 * ledger's internal units: whole millionths, rounded down. */
async function line(node: ScenarioNode) {
  const { externalAssets } = await node.rpc.call("listLedgers", {});
  const holdings = externalAssets.flatMap((asset) => asset.holdings);
  assert(
    !holdings.some((holding) => holding.assetCode === "RLUSD"),
    `${node.name} has an RLUSD line`,
  );
  const holding = holdings.find((holding) => holding.assetCode === "DSUSD");
  assert(holding, `${node.name} shows no DSUSD line`);
  const scale = BigInt(holding.assetScale);
  const micros = holding.amount / 10n ** (scale - 6n);
  return { holding, units: micros * UNIT };
}

/** Whether the node follows the ledger, and its book holds its line. */
async function inStep(node: ScenarioNode): Promise<boolean> {
  const view = await ledger(node);
  if (view.connection !== LedgerConnection.SYNCED) return false;
  return view.onLedgerBalance === (await line(node)).units;
}

/** The deposits and withdrawals the node booked on the ledger, as `+` or
 * `-` and the amount. */
async function moves(node: ScenarioNode): Promise<string[]> {
  const { transfers } = await node.rpc.call("listTransfers", { limit: 1000 });
  return transfers
    .filter((transfer) =>
      transfer.reference.case === "other" &&
      ["deposit", "withdrawal"].includes(transfer.reference.value) &&
      (transfer.debitPath.startsWith(`${LEDGER}:`) ||
        transfer.creditPath.startsWith(`${LEDGER}:`))
    )
    .map((transfer) =>
      `${transfer.reference.value === "deposit" ? "+" : "-"}${transfer.amount}`
    )
    .sort();
}

test(
  "dsusd-owner-book books the mint, keeps booking while the peg guard pauses it, and books the issuer's clawback as a withdrawal",
  { skip: missing !== undefined },
  async () => {
    const scenario = await Scenario.temporary("dsusd-owner-book", {
      dassie: DASSIE,
      fakeLedger: FAKE_LEDGER,
      portOffset: PORT_OFFSET,
    });
    try {
      const [node] = scenario.nodes;
      // The mint is booked: the book holds its line, and the owner the
      // DSUSD minted.
      await scenario.until(
        "minted",
        async () =>
          (await inStep(node)) && (await ledger(node)).ownerBalance === MINTED,
        120_000,
      );
      // The ledger names DSUSD's issuer, and the line is to it.
      const { holding } = await line(node);
      assertEquals(holding.issuer, ISSUER);
      assertEquals((await ledger(node)).issuer, ISSUER);
      // An owner book peers with nobody, and offers no peering.
      assertEquals((await node.rpc.call("listPeers", {})).peers, []);

      const guard = async () => (await ledger(node)).peg;
      await scenario.until(
        "read at a dollar",
        async () => {
          const peg = await guard();
          return peg?.paused === PegPause.UNSPECIFIED &&
            peg.pricePpm === 1_000_000n;
        },
        60_000,
      );

      // The pool prices DSUSD 5% under a dollar: once every reading has
      // been off for the hold, the node pauses the book's conversions and
      // says why.
      const par = poolDrops(POOL_VALUE, DEV_RATES.XRP);
      await scenario.askIssuer({
        action: "amm",
        token: "DSUSD",
        drops: `${par * 95n / 100n}`,
        value: POOL_VALUE,
        fee: POOL_FEE,
      });
      const offPeg = async () => {
        const peg = await guard();
        return peg?.paused === PegPause.OFF_PEG && peg.pricePpm === 950_000n;
      };
      await scenario.until("paused off its peg", offPeg, 60_000);

      // What arrives on the line is still booked while paused.
      const owned = (await ledger(node)).ownerBalance;
      const { address } = (await node.rpc.call("listLedgers", {}))
        .externalAssets[0];
      await scenario.askIssuer({
        token: "DSUSD",
        action: "mint",
        destination: address,
        value: "1",
      });
      await scenario.until(
        "the deposit booked while paused",
        async () =>
          (await ledger(node)).ownerBalance === owned + DOLLAR &&
          (await inStep(node)),
        60_000,
      );
      assert(await offPeg(), "still paused");

      // The issuer freezes the line: its owner sees it, as the ledger
      // says, and the issuer lifts it again.
      await scenario.askIssuer({
        token: "DSUSD",
        action: "freeze",
        account: address,
        on: true,
      });
      await scenario.until(
        "the freeze shown",
        async () => (await line(node)).holding.frozen,
        60_000,
      );
      await scenario.askIssuer({
        token: "DSUSD",
        action: "freeze",
        account: address,
        on: false,
      });
      await scenario.until(
        "the freeze lifted",
        async () => !(await line(node)).holding.frozen,
        60_000,
      );

      // A clawback lowers the line without a Payment: the book takes it as
      // a withdrawal, and still holds its line.
      const before = (await ledger(node)).ownerBalance;
      const held = (await line(node)).units;
      await scenario.askIssuer({
        token: "DSUSD",
        action: "clawback",
        account: address,
        value: "2",
      });
      await scenario.until(
        "the clawback booked",
        async () =>
          (await line(node)).units === held - CLAWED && (await inStep(node)),
        60_000,
      );
      assertEquals((await ledger(node)).ownerBalance, before - CLAWED);
      // Each booked once: the two mints and the clawback.
      assertEquals(
        await moves(node),
        [`+${DOLLAR}`, `+${MINTED}`, `-${CLAWED}`].sort(),
      );
    } finally {
      const failures = await scenario.stop();
      assertEquals(failures, [], failures.join("\n"));
    }
  },
);
