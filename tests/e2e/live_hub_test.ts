/**
 * End to end (spec/areas/testing.md#end-to-end): a smoke test of hubs on
 * mainnet in the live realm, on the shipped binary. The `live-hub`
 * scenario's nodes run in the live realm by the dev switch
 * (`DASSIE_DEV_LIVE_REALM`), every account funded by a deposit, as mainnet
 * has no faucet. Four validators, whose validator files name mainnet, run
 * an XRP hub and an RLUSD hub that their sponsor founds through its
 * owner's RPC on the fake ledger's mainnet; two members join both with the
 * founded rules in their config, from their owner books of XRP and RLUSD,
 * and peer over both. Their hub ledgers are of the live realm, and a
 * client of one, the JS STREAM sender over BTP, pays the other in XRP over
 * the XRP hub, under `g.`.
 *
 * That is all: what the members do over the hubs, the RLUSD payment back
 * and the books included, is the daemon tests'
 * (`hub_owner_books.rs`,
 * `live_realm_members_pay_over_mainnet_hubs_of_xrp_and_rlusd`, on paused
 * time: spec/areas/testing.md#members-of-settlement-hubs-on-paused-time).
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { Realm } from "@dassie/rpc/proto";

import {
  DASSIE,
  FAKE_LEDGER,
  missingScenarioTools,
  PORT_OFFSETS,
} from "./harness.ts";
import { hubMembers, hubOf, joinedHubs, ledger, pay } from "./settling.ts";
import { hubLedger, Scenario } from "@dassie/dev/scenario";

/** 2 XRP: twenty of the largest packets, so the payer funds its hold
 * toward the payee over the hub as it pays. */
const XRP_PAYMENT = 2_000_000_000n;
/** Clear of every other file's ports (`PORT_OFFSETS`, which ports_test.ts checks). */
const PORT_OFFSET = PORT_OFFSETS["live_hub_test.ts"];

const missing = await missingScenarioTools();

test(
  "live-hub founds an XRP hub and an RLUSD hub on mainnet in the live realm, and a STREAM client pays over one",
  { skip: missing !== undefined },
  async () => {
    const scenario = await Scenario.temporary("live-hub", {
      dassie: DASSIE,
      fakeLedger: FAKE_LEDGER,
      portOffset: PORT_OFFSET,
    });
    try {
      const xrpHub = hubOf(scenario, "hub-xrpl-xrp");
      const rlusdHub = hubOf(scenario, "hub-xrpl-rlusd");
      const [xrpLedger, rlusdLedger] = [xrpHub, rlusdHub].map(hubLedger);
      const members = hubMembers(scenario, xrpHub);
      assertEquals(hubMembers(scenario, rlusdHub), members);
      const [first, second] = members;
      assert(first && second, "two members");
      await joinedHubs(scenario);

      // The hubs are on mainnet: the members' ledgers of them are of the
      // live realm.
      for (const node of members) {
        for (const id of [xrpLedger, rlusdLedger]) {
          assertEquals((await ledger(node, id)).realm, Realm.LIVE, id);
        }
      }

      const address = await pay(
        scenario,
        [first, second],
        xrpLedger,
        XRP_PAYMENT,
        xrpLedger,
      );
      assert(address.startsWith("g."), address);
      const { payments } = await second.rpc.call("listIncomingPayments", {});
      const incoming = payments.find((p) => p.destinationAccount === address);
      assert(incoming, "no incoming payment");
      assertEquals(incoming.ledger, xrpLedger);
      assertEquals(incoming.receivedAmount, XRP_PAYMENT, "what arrived");
    } finally {
      const failures = await scenario.stop();
      assertEquals(failures, [], failures.join("\n"));
    }
  },
);
