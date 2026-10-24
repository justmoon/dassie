/**
 * End to end (spec/areas/testing.md#end-to-end): the nodes prove what they
 * book against the fake ledger's validators, which sign every ledger for
 * real (spec/designs/xrpl-validation.md#1-the-rule). On the
 * `xrpl-settlement` scenario, funded, peered and settled, the fake ledger's
 * server lies, then serves no proofs, then two of its five validators sign
 * nothing, short of a quorum. Each time, every node waits, says why in its
 * ledger, and books nothing, not even a deposit that really arrived
 * meanwhile; once the server is honest and the validators sign again, it
 * books the deposit once, and a payment settles on the ledger as before.
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { LedgerConnection } from "@dassie/rpc/proto";

import { DASSIE, FAKE_LEDGER, missingScenarioTools } from "./harness.ts";
import { CREDIT, ledger, pay, peerAccounts, settled } from "./settling.ts";
import { type LieKind, Scenario } from "@dassie/dev/scenario";

const LEDGER = "xrpl-testnet+xrp";
/** 5 XRP: over four times what a peer extends, so d1 must settle. */
const PAYMENT = 5_000_000_000n;
/** What someone sends d1 while it cannot prove: 3 XRP, in drops. */
const DEPOSIT_DROPS = 3_000_000n;
/** The same, in the ledger's internal units. */
const DEPOSIT = DEPOSIT_DROPS * 1_000n;
/**
 * Clear of every other file's ports, which run beside these, as
 * market_maker_targets_test.ts lays them out: at 12000 this file's nodes
 * take 16001 up and 17001 up on TCP, and 18001 up for their feeds on UDP,
 * and its fake ledger 22200 and 22201.
 */
const PORT_OFFSET = 12_000;

const missing = await missingScenarioTools();

/** What each node's books hold of its account, and its owner's funds. */
async function books(scenario: Scenario): Promise<Map<string, bigint[]>> {
  const out = new Map<string, bigint[]>();
  for (const node of scenario.nodes) {
    const view = await ledger(node, LEDGER);
    out.set(node.name, [view.onLedgerBalance, view.ownerBalance]);
  }
  return out;
}

/** Whether every node waits for its ledger, for `why`. */
async function waiting(scenario: Scenario, why: string): Promise<boolean> {
  for (const node of scenario.nodes) {
    if ((await ledger(node, LEDGER)).connectionProblem !== why) return false;
  }
  return true;
}

/** What each fault is, how to end it, and why the nodes say they wait. */
const FAULTS: {
  what: string;
  why: string;
  start: (scenario: Scenario) => Promise<void>;
  end: (scenario: Scenario) => Promise<void>;
}[] = [
  ...([
    ["altered-object", "the server's answers did not prove"],
    ["no-proofs", "the server serves no proofs; name a proof server"],
  ] as [LieKind, string][]).map(([kind, why]) => ({
    what: `the server tells ${kind}`,
    why,
    start: (scenario: Scenario) => scenario.lie(kind),
    end: (scenario: Scenario) => scenario.lie("honest"),
  })),
  {
    what: "two validators sign nothing",
    why: "no quorum of validators signed the server's validated ledger",
    start: async (scenario) => {
      for (const validator of [1, 2]) {
        await scenario.askValidators({
          action: "withhold",
          validator,
          on: true,
        });
      }
    },
    end: async (scenario) => {
      for (const validator of [1, 2]) {
        await scenario.askValidators({
          action: "withhold",
          validator,
          on: false,
        });
      }
    },
  },
];

test(
  "a lying server, a server without proofs and a quorum withheld get nothing booked",
  { skip: missing !== undefined },
  async () => {
    const scenario = await Scenario.temporary("xrpl-settlement", {
      dassie: DASSIE,
      fakeLedger: FAKE_LEDGER,
      portOffset: PORT_OFFSET,
    });
    try {
      const funded = async () => {
        for (const node of scenario.nodes) {
          const view = await ledger(node, LEDGER);
          if (view.connection !== LedgerConnection.SYNCED) return false;
          if (view.ownerBalance <= 0n) return false;
        }
        return true;
      };
      await scenario.until("funded", funded, 120_000);
      await scenario.until("peered", () => scenario.converged(), 120_000);
      await scenario.until(
        "paid in advance",
        () => settled(scenario, LEDGER),
        120_000,
      );
      const [d1, d2] = scenario.nodes;
      const address = (await ledger(d1, LEDGER)).depositAddress;

      for (const fault of FAULTS) {
        const kept = await books(scenario);
        await fault.start(scenario);
        await scenario.deposit(address, DEPOSIT_DROPS);
        await scenario.until(
          `waiting while ${fault.what}`,
          () => waiting(scenario, fault.why),
          90_000,
        );
        assertEquals(await books(scenario), kept, `while ${fault.what}`);
        await fault.end(scenario);
        // Honest again, d1 books the deposit once, and nothing else moved.
        const booked = async () => {
          const now = await books(scenario);
          const [onLedger, owner] = now.get(d1.name)!;
          const [keptOnLedger, keptOwner] = kept.get(d1.name)!;
          const d2Kept = kept.get(d2.name)!;
          return onLedger === keptOnLedger + DEPOSIT &&
            owner === keptOwner + DEPOSIT &&
            now.get(d2.name)!.every((held, at) => held === d2Kept[at]) &&
            await settled(scenario, LEDGER);
        };
        await scenario.until(`booked after ${fault.what}`, booked, 120_000);
        assertEquals((await ledger(d1, LEDGER)).connectionProblem, "");
      }

      // A payment settles on the ledger as before.
      const before = await ledger(d1, LEDGER);
      await pay(scenario, [d1, d2], LEDGER, PAYMENT);
      await scenario.until(
        "settled",
        () => settled(scenario, LEDGER),
        120_000,
      );
      const after = await ledger(d1, LEDGER);
      assert(
        after.onLedgerBalance < before.onLedgerBalance,
        "d1 settled on the ledger",
      );
      const owes = (await peerAccounts(scenario, LEDGER)).get("d1>d2")!;
      assert(owes <= CREDIT, `d1 still owes ${owes}`);
    } finally {
      const failures = await scenario.stop();
      assertEquals(failures, [], failures.join("\n"));
    }
  },
);
