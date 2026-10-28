/**
 * End to end (spec/areas/testing.md#end-to-end): the `two-hubs` scenario's
 * nodes found a hub of XRP and a hub of DSUSD on the fake XRP Ledger, and
 * a payment crosses from one to the other. Four validators run both hubs,
 * which their sponsor founds through its owner's RPC, the DSUSD hub with
 * terms that name the token. One member is in the XRP hub alone, one in
 * the DSUSD hub alone, and two are in both; each funds its accounts in its
 * hubs from its owner books of XRP and DSUSD, which peer with nobody, and
 * the two in both peer over both hubs, a method each.
 *
 * A client of the member in the XRP hub alone pays a receiver in DSUSD of
 * the member in the DSUSD hub alone, through a member of both, which
 * converts at the dev rate. What arrived is what was sent, in dollars,
 * less what the connectors take; each node's books show the payment, in
 * step with what its accounts hold.
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";

import {
  DASSIE,
  FAKE_LEDGER,
  missingScenarioTools,
  PORT_OFFSETS,
} from "./harness.ts";
import {
  booksOverHubs,
  HUB_WITHIN_MS,
  hubMembers,
  hubOf,
  joinedHubs,
  pay,
  peerLedgers,
  settledOverHubs,
} from "./settling.ts";
import { DEV_RATES, hubLedger, Scenario } from "@dassie/dev/scenario";

/** 1 XRP: ten of the largest packets, so the payer funds its holds toward
 * the members of both hubs as it pays. */
const PAYMENT = 1_000_000_000n;
/** Clear of every other file's ports (`PORT_OFFSETS`, which ports_test.ts checks). */
const PORT_OFFSET = PORT_OFFSETS["hubs_cross_test.ts"];

const missing = await missingScenarioTools();

test(
  "two-hubs founds an XRP hub and a DSUSD hub, and a payment crosses from one to the other through a member of both",
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
      const [payer] = inXrp.filter((node) => !both.includes(node));
      const [payee] = inDsusd.filter((node) => !both.includes(node));
      const members = [...new Set([...inXrp, ...inDsusd])];
      assertEquals(both.length, 2, "two members in both hubs");
      assert(payer && payee, "a member in each hub alone");
      await joinedHubs(scenario);

      // The pair in both hubs peers over both, with a peer account on each
      // hub's ledger; the others over the one hub they share with each.
      const [first, second] = both;
      for (const [node, peer] of [[first, second], [second, first]]) {
        assertEquals(
          await peerLedgers(node, peer),
          [dsusdLedger, xrpLedger].sort(),
          `${node.name} with ${peer.name}`,
        );
      }
      for (const member of both) {
        assertEquals(await peerLedgers(payer, member), [xrpLedger]);
        assertEquals(await peerLedgers(payee, member), [dsusdLedger]);
      }
      assertEquals(await peerLedgers(payer, payee), [], "no hub in common");

      const before = await Promise.all(
        members.map((node) => booksOverHubs(scenario, node)),
      );
      const address = await pay(
        scenario,
        [payer, payee],
        xrpLedger,
        PAYMENT,
        dsusdLedger,
      );
      const { payments } = await payee.rpc.call("listIncomingPayments", {});
      const incoming = payments.find((p) => p.destinationAccount === address);
      assert(incoming, "no incoming payment");
      assertEquals(incoming.ledger, dsusdLedger);
      // XRP at the dev rate, in dollars: a member of both converts it at a
      // rate its inventory moves from spot, either way, by at most the
      // curve's largest `k`, 10% (spec/decisions/passive-rebalancing.md),
      // and the connectors keep their fees and spread.
      const dollars = PAYMENT * 1_000_000n /
        BigInt(Math.round(Number(DEV_RATES.XRP) * 1_000_000));
      const received = incoming.receivedAmount;
      assert(
        received > dollars * 85n / 100n && received < dollars * 110n / 100n,
        `${received} arrived of ${dollars} sent`,
      );
      await scenario.until(
        "settled",
        () => settledOverHubs(scenario),
        HUB_WITHIN_MS,
      );

      // Each node's books show it, in what its owner has, a hub and the
      // owner book of its asset taken together, as the node moves funds
      // between them by itself: the payer's owner paid what was sent in
      // XRP, and the fees of its moves and operations; the payee's got
      // dollars, what arrived less those of its own; and the owners of the
      // members of both, taken together, have more XRP and fewer dollars,
      // as whichever of them the payment crossed took the one and paid
      // the other. What their accounts hold need not move: the payment may
      // take no more from a member's holds toward the payee than they held
      // before it, and a payee asks for more only once they run low.
      const after = await Promise.all(
        members.map((node) => booksOverHubs(scenario, node)),
      );
      const XRP_BOOKS = [xrpLedger, "xrpl-testnet-xrp+xrp"];
      const DOLLAR_BOOKS = [dsusdLedger, "xrpl-testnet-dsusd+usd"];
      // What the owner of `node` has on the books `ids`, after less before.
      const moved = (node: typeof payer, ids: readonly string[]) => {
        const index = members.indexOf(node);
        let sum = 0n;
        for (const id of ids) {
          sum += (after[index][id]?.[1] ?? 0n) - (before[index][id]?.[1] ?? 0n);
        }
        return sum;
      };
      const spent = -moved(payer, XRP_BOOKS);
      assert(spent >= PAYMENT, `the payer's owner spent ${spent}`);
      const got = moved(payee, DOLLAR_BOOKS);
      assert(got > 0n && got <= received, `the payee's owner got ${got}`);
      const tookXrp = both.reduce((sum, m) => sum + moved(m, XRP_BOOKS), 0n);
      const paidDollars = -both.reduce(
        (sum, m) => sum + moved(m, DOLLAR_BOOKS),
        0n,
      );
      assert(tookXrp > 0n, `the members of both took ${tookXrp} in XRP`);
      assert(paidDollars > 0n, `the members of both paid ${paidDollars}`);
    } finally {
      const failures = await scenario.stop();
      assertEquals(failures, [], failures.join("\n"));
    }
  },
);
