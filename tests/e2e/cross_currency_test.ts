/**
 * End to end (spec/areas/testing.md#end-to-end): a smoke test of a
 * settlement hub on the shipped binary, on the `cross-currency` scenario.
 * Four validators, each given its hub by the validator file the scenario
 * writes (`DASSIE_VALIDATOR_CONFIG`), run the hub their sponsor founds
 * through its owner's RPC on the fake ledger; d2 and d3 join it from their
 * owner books of XRP and peer over it, d1 and d2 peer on the stub with
 * unlimited trust. A client of d1's, the JS STREAM sender over BTP, pays
 * dollars to a receiver of d3's on the hub's ledger: d2 converts them, and
 * funds its hold toward d3 over the hub as d3 asks, so XRP arrives.
 *
 * That is all: what the members do over the hub is the daemon tests'
 * (spec/areas/testing.md#members-of-settlement-hubs-on-paused-time),
 * which check it on paused time. `hub_members.rs` checks a member's wallet
 * address, a payment over a hub, the books, a restart and a node added
 * later; `hub_owner_books.rs` the owner books and the moves between them
 * and the hub; `hub_cross.rs` dollars on the stub arriving as XRP over a
 * hub, with the converting node's fee, the holds and a restart, and a
 * payment from one hub to another; `hub_founding.rs` the founding.
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";

import {
  DASSIE,
  FAKE_LEDGER,
  missingScenarioTools,
  PORT_OFFSETS,
} from "./harness.ts";
import { fundedInHub, HUB_WITHIN_MS, pay, peersOverHub } from "./settling.ts";
import {
  DEV_RATES,
  HUB_LEDGER,
  Scenario,
  type ScenarioNode,
} from "@dassie/dev/scenario";

const USD = "stub+usd";
/** A dollar at the ledger's scale of 9: two XRP at the dev rate, twenty
 * of the largest packets, so d2 funds its hold toward d3 as it pays. */
const PAYMENT = 1_000_000_000n;
const PORT_OFFSET = PORT_OFFSETS["cross_currency_test.ts"];

const missing = await missingScenarioTools();

/** Whether `node` peers with `peer` on the stub. */
async function peersOnStub(
  node: ScenarioNode,
  peer: ScenarioNode,
): Promise<boolean> {
  const { peers } = await node.rpc.call("listPeers", {});
  return peers.some((p) =>
    p.node === peer.id && p.state === "peered" &&
    p.methods.some((m) => m.scheme === "stub")
  );
}

/**
 * d2 and d3 joined the hub and peer over it, d2 holds funds toward d3
 * there, d1 and d2 peer on the stub, d1 routes to d3, and d2 has a fresh
 * rate for XRP.
 */
async function ready(scenario: Scenario): Promise<boolean> {
  const [d1, d2, d3] = scenario.nodes;
  for (const node of [d2, d3]) if (!(await fundedInHub(node))) return false;
  if (!(await peersOnStub(d1, d2)) || !(await peersOnStub(d2, d1))) {
    return false;
  }
  if (!(await peersOverHub(d2, d3)) || !(await peersOverHub(d3, d2))) {
    return false;
  }
  // d1 routes to d3, through d2: until then STREAM's probe of the rate
  // finds no path.
  const { routes } = await d1.rpc.call("listRoutes", {});
  if (!routes.some((route) => route.node === d3.id)) return false;
  const { hubs } = await d2.rpc.call("getHubStatus", {});
  const holds = hubs.some((hub) =>
    hub.paying.some((hold) => hold.peer === d3.id && hold.amount > 0n)
  );
  const { rates } = await d2.rpc.call("listLedgers", {});
  return holds &&
    rates.some((rate) => rate.assetCode === "XRP" && !rate.stale);
}

test(
  "cross-currency founds a hub, and a STREAM client pays dollars across it as XRP",
  { skip: missing !== undefined },
  async () => {
    const scenario = await Scenario.temporary("cross-currency", {
      dassie: DASSIE,
      fakeLedger: FAKE_LEDGER,
      portOffset: PORT_OFFSET,
    });
    try {
      const [d1, , d3] = scenario.nodes;
      await scenario.until(
        "joined and peered",
        () => ready(scenario),
        HUB_WITHIN_MS,
      );
      const address = await pay(scenario, [d1, d3], USD, PAYMENT, HUB_LEDGER);
      const { payments } = await d3.rpc.call("listIncomingPayments", {});
      const incoming = payments.find((p) => p.destinationAccount === address);
      assert(incoming, "no incoming payment");
      assertEquals(incoming.ledger, HUB_LEDGER);
      assertEquals(incoming.assetCode, "XRP");
      // Dollars at the dev rate, in XRP, moved by d2's inventory curve by
      // at most 10% either way, less its fee and spread.
      const xrp = PAYMENT * BigInt(DEV_RATES.XRP);
      const received = incoming.receivedAmount;
      assert(
        received > xrp * 85n / 100n && received < xrp * 110n / 100n,
        `${received} arrived of ${xrp} sent`,
      );
    } finally {
      const failures = await scenario.stop();
      assertEquals(failures, [], failures.join("\n"));
    }
  },
);
