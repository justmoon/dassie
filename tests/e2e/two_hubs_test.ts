/**
 * End to end (spec/areas/testing.md#end-to-end): a smoke test of the
 * `two-hubs` scenario, the one whose members are in different numbers of
 * hubs. Its sponsor founds an XRP hub and a DSUSD hub through its owner's
 * RPC; the two members of both join first, and the members of one hub
 * alone only once those two peered (`Scenario`'s staged join,
 * spec/areas/dev.md#scenarios). Every member then funds its account in
 * each of its hubs and peers over each hub it shares with another: the
 * two members of both over both, each other member over its one hub, and
 * the members of one hub alone, which share none, not at all.
 *
 * No payment: what the members do over the hubs, a payment from one hub to
 * the other through a member of both included, is the daemon tests'
 * (`hub_cross.rs`, on paused time:
 * spec/areas/testing.md#members-of-settlement-hubs-on-paused-time).
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";

import {
  DASSIE,
  FAKE_LEDGER,
  missingScenarioTools,
  PORT_OFFSETS,
} from "./harness.ts";
import { hubMembers, hubOf, joinedHubs, peerLedgers } from "./settling.ts";
import { hubLedger, Scenario } from "@dassie/dev/scenario";

/** Clear of every other file's ports (`PORT_OFFSETS`, which ports_test.ts checks). */
const PORT_OFFSET = PORT_OFFSETS["two_hubs_test.ts"];

const missing = await missingScenarioTools();

test(
  "two-hubs founds an XRP hub and a DSUSD hub, and its members join and peer over the hubs they share",
  { skip: missing !== undefined },
  async () => {
    const scenario = await Scenario.temporary("two-hubs", {
      dassie: DASSIE,
      fakeLedger: FAKE_LEDGER,
      portOffset: PORT_OFFSET,
    });
    try {
      const xrpHub = hubOf(scenario, "hub-xrpl-testnet-xrp");
      const dsusdHub = hubOf(scenario, "hub-xrpl-testnet-dsusd");
      const [xrpLedger, dsusdLedger] = [xrpHub, dsusdHub].map(hubLedger);
      const inXrp = hubMembers(scenario, xrpHub);
      const inDsusd = hubMembers(scenario, dsusdHub);
      const both = inXrp.filter((node) => inDsusd.includes(node));
      const [xrpAlone] = inXrp.filter((node) => !both.includes(node));
      const [dsusdAlone] = inDsusd.filter((node) => !both.includes(node));
      assertEquals(both.length, 2, "two members in both hubs");
      assert(xrpAlone && dsusdAlone, "a member in each hub alone");
      await joinedHubs(scenario);

      const [first, second] = both;
      for (const [node, peer] of [[first, second], [second, first]]) {
        assertEquals(
          await peerLedgers(node, peer),
          [dsusdLedger, xrpLedger].sort(),
          `${node.name} with ${peer.name}`,
        );
      }
      for (const member of both) {
        assertEquals(await peerLedgers(xrpAlone, member), [xrpLedger]);
        assertEquals(await peerLedgers(dsusdAlone, member), [dsusdLedger]);
      }
      assertEquals(
        await peerLedgers(xrpAlone, dsusdAlone),
        [],
        "no hub in common",
      );
    } finally {
      const failures = await scenario.stop();
      assertEquals(failures, [], failures.join("\n"));
    }
  },
);
