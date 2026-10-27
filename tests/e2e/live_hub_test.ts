/**
 * End to end (spec/areas/testing.md#end-to-end): the `live-hub` scenario's
 * nodes run in the live realm, by the dev switch, and found a hub of XRP
 * and a hub of RLUSD on the fake XRP Ledger's mainnet, where every account
 * is funded by a deposit, as mainnet has no faucet. Four validators, whose
 * files name mainnet, run both hubs, which their sponsor founds through
 * its owner's RPC on its realm's network, funding the escrows from its
 * `xrpl-xrp` book, the RLUSD hub's terms naming RLUSD's issuer. Two members
 * join both with `hub-xrpl-xrp` and `hub-xrpl-rlusd`, the founded rules in
 * their config, since no mainnet hub is built in; each funds its accounts
 * from its owner books of XRP and RLUSD, which peer with nobody, and the
 * two peer over both hubs, a method each.
 *
 * A node of the live realm is offered no mainnet hub's scheme while none
 * is built in. A client of one member pays the other's receiver in XRP
 * over the XRP hub, and the other pays back in RLUSD over the RLUSD hub:
 * each payment keeps its ledger, so what arrived is what was sent, and
 * each owner's books show it, in step with what its accounts hold.
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { Realm } from "@dassie/rpc/proto";

import { DASSIE, FAKE_LEDGER, missingScenarioTools } from "./harness.ts";
import {
  booksOverHubs,
  HUB_WITHIN_MS,
  hubMembers,
  hubOf,
  joinedHubs,
  ledger,
  pay,
  peerLedgers,
  settledOverHubs,
} from "./settling.ts";
import { hubLedger, Scenario } from "@dassie/dev/scenario";

/** 2 XRP: twenty of the largest packets, so the payer funds its hold
 * toward the payee over the hub as it pays. */
const XRP_PAYMENT = 2_000_000_000n;
/** 1 RLUSD, at the ledger's scale of 9. */
const RLUSD_PAYMENT = 1_000_000_000n;
/**
 * Clear of every other file's ports, which run beside these, as
 * market_maker_targets_test.ts lays them out: at 3000 this file's nodes
 * take 7001 up and 8001 up on TCP, 8001 up for QUIC and 9001 up for their
 * feeds on UDP, and its fake ledger and rate server 13200, 13201 and
 * 13210 on TCP.
 */
const PORT_OFFSET = 3_000;

const missing = await missingScenarioTools();

test(
  "live-hub founds an XRP hub and an RLUSD hub on mainnet in the live realm, and its members pay each other over both",
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
      assertEquals([xrpLedger, rlusdLedger], [
        "hub-xrpl-xrp+xrp",
        "hub-xrpl-rlusd+usd",
      ]);
      const members = hubMembers(scenario, xrpHub);
      assertEquals(hubMembers(scenario, rlusdHub), members);
      const [first, second] = members;
      assert(first && second, "two members");

      // While no mainnet hub is built in, a node of the live realm is
      // offered mainnet's books alone, and no hub's scheme.
      const validator = scenario.nodes.find((node) =>
        !members.includes(node) && node.index !== xrpHub.sponsor
      );
      assert(validator, "a validator that sponsors nothing");
      const { schemes } = await validator.rpc.call("listSettlementSchemes", {});
      assertEquals(schemes.map((scheme) => scheme.id).sort(), [
        "xrpl-rlusd",
        "xrpl-xrp",
      ]);

      await joinedHubs(scenario);

      // The hubs are on mainnet: the members' ledgers of them are of the
      // live realm, and the two peer over both, a method each.
      for (const node of members) {
        for (const id of [xrpLedger, rlusdLedger]) {
          assertEquals((await ledger(node, id)).realm, Realm.LIVE, id);
        }
      }
      for (const [node, peer] of [[first, second], [second, first]]) {
        assertEquals(
          await peerLedgers(node, peer),
          [rlusdLedger, xrpLedger].sort(),
          `${node.name} with ${peer.name}`,
        );
      }

      const before = await Promise.all(
        members.map((node) => booksOverHubs(scenario, node)),
      );
      // Each payment keeps the ledger it came in on, so it converts
      // nowhere: what arrived is what was sent.
      for (
        const [payer, payee, id, amount] of [
          [first, second, xrpLedger, XRP_PAYMENT],
          [second, first, rlusdLedger, RLUSD_PAYMENT],
        ] as const
      ) {
        const address = await pay(scenario, [payer, payee], id, amount, id);
        assert(address.startsWith("g."), address);
        const { payments } = await payee.rpc.call("listIncomingPayments", {});
        const incoming = payments.find((p) => p.destinationAccount === address);
        assert(incoming, `no incoming payment on ${id}`);
        assertEquals(incoming.ledger, id);
        assertEquals(incoming.receivedAmount, amount, `what arrived on ${id}`);
      }
      await scenario.until(
        "settled",
        () => settledOverHubs(scenario),
        HUB_WITHIN_MS,
      );

      // Each owner's books show both payments, in what it has on a hub and
      // the owner book of its asset taken together, as the node moves funds
      // between them by itself: the payer of each paid what was sent, and
      // the fees of its moves and operations; the payee got what arrived,
      // less those of its own.
      const after = await Promise.all(
        members.map((node) => booksOverHubs(scenario, node)),
      );
      const moved = (index: number, ids: readonly string[]) =>
        ids.reduce(
          (sum, id) =>
            sum + (after[index][id]?.[1] ?? 0n) -
            (before[index][id]?.[1] ?? 0n),
          0n,
        );
      const xrpBooks = [xrpLedger, "xrpl-xrp+xrp"];
      const dollarBooks = [rlusdLedger, "xrpl-rlusd+usd"];
      const [firstXrp, secondXrp] = [0, 1].map((i) => moved(i, xrpBooks));
      const [firstUsd, secondUsd] = [0, 1].map((i) => moved(i, dollarBooks));
      assert(-firstXrp >= XRP_PAYMENT, `${first.name} paid ${-firstXrp} XRP`);
      assert(
        secondXrp > 0n && secondXrp <= XRP_PAYMENT,
        `${second.name} got ${secondXrp} XRP`,
      );
      assert(
        -secondUsd >= RLUSD_PAYMENT,
        `${second.name} paid ${-secondUsd} RLUSD`,
      );
      assert(
        firstUsd > 0n && firstUsd <= RLUSD_PAYMENT,
        `${first.name} got ${firstUsd} RLUSD`,
      );
    } finally {
      const failures = await scenario.stop();
      assertEquals(failures, [], failures.join("\n"));
    }
  },
);
