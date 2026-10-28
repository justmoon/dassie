/**
 * End to end (spec/areas/testing.md#end-to-end): the `hub` scenario's
 * nodes found a settlement hub on the fake XRP Ledger, and its members
 * settle over it. Four validators run the hub their sponsor founds through
 * its owner's RPC; the two members fund their accounts in it from their
 * owner books of XRP, which peer with nobody, and peer over the hub alone.
 *
 * The payee's one wallet address for XRP books on the hub's ledger, where
 * what it receives stays, and deposits there name its account by a tag. A
 * client of one member pays the other's receiver on that ledger twenty of
 * the largest packets, so the payer funds its hold toward the payee over the hub as it
 * pays; what arrived is what was sent, each member's books hold what its
 * accounts hold, and both members' books are unchanged once they restart.
 * The validators keep running meanwhile, so the hub does too. A node added
 * then joins the hub as a member, funds its account and peers over it.
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
  assertPeersOverHubAlone,
  fundedInHub,
  HUB_WITHIN_MS,
  hubBooks,
  joinedHub,
  ledger,
  members,
  ownerFunds,
  pay,
  peerAccounts,
  peersOverHub,
  settledOverHub,
} from "./settling.ts";
import { FundsKept } from "@dassie/rpc/proto";
import { HUB_LEDGER, HUB_SCHEME, Scenario } from "@dassie/dev/scenario";

/** 2 XRP: twenty of the largest packets, so the payer funds its hold
 * toward the payee over the hub as it pays. */
const PAYMENT = 2_000_000_000n;
/** Clear of every other file's ports (`PORT_OFFSETS`, which ports_test.ts checks). */
const PORT_OFFSET = PORT_OFFSETS["hub_test.ts"];

const missing = await missingScenarioTools();

test(
  "hub founds a settlement hub, its members settle over it across restarts, and an added node joins it",
  { skip: missing !== undefined },
  async () => {
    const scenario = await Scenario.temporary("hub", {
      dassie: DASSIE,
      fakeLedger: FAKE_LEDGER,
      portOffset: PORT_OFFSET,
    });
    try {
      const [payer, payee] = members(scenario);
      await joinedHub(scenario);

      // The payee has one wallet address for XRP, which books on the hub's
      // ledger, where what its peers pay arrives, and none names the hub;
      // what it receives stays there, so a deposit goes to the escrow
      // with its account's tag (spec/areas/payments.md#wallet-addresses).
      const { walletAddresses } = await payee.rpc.call(
        "listWalletAddresses",
        {},
      );
      const xrp = walletAddresses.find((address) => address.name === "xrp");
      assert(xrp, "no XRP wallet address");
      assertEquals(xrp.ledger, HUB_LEDGER);
      assertEquals(xrp.fundsKept, FundsKept.HUB);
      assertEquals(xrp.depositLedger, HUB_LEDGER);
      assert(
        !walletAddresses.some((address) => address.url.includes("/pay/hub-")),
        "an address names the hub",
      );
      const hubAccount = await ledger(payee, HUB_LEDGER);
      assert(hubAccount.depositTag, "no destination tag for deposits");

      const [paid, received] = await Promise.all(
        [payer, payee].map(ownerFunds),
      );
      const address = await pay(
        scenario,
        [payer, payee],
        HUB_LEDGER,
        PAYMENT,
        xrp.ledger,
      );
      const { payments } = await payee.rpc.call("listIncomingPayments", {});
      const incoming = payments.find((p) => p.destinationAccount === address);
      assert(incoming, "no incoming payment");
      assertEquals(incoming.ledger, HUB_LEDGER);
      assertEquals(incoming.receivedAmount, PAYMENT, "what arrived");
      await scenario.until(
        "settled",
        () => settledOverHub(scenario),
        HUB_WITHIN_MS,
      );
      // The payer's owner paid what was sent and the fees of its moves and
      // operations; the payee's got what arrived, less those of its own.
      const spent = paid - await ownerFunds(payer);
      const got = await ownerFunds(payee) - received;
      assert(spent >= PAYMENT, `the payer's owner spent ${spent}`);
      assert(got > 0n && got <= PAYMENT, `the payee's owner got ${got}`);

      // Restarted, both members read their accounts again and book nothing
      // twice.
      const kept = await Promise.all([payer, payee].map(hubBooks));
      const accounts = await peerAccounts(scenario, HUB_LEDGER);
      for (const node of [payer, payee]) {
        const { code, output } = (await node.stop())!;
        assertEquals(code, 0, output);
      }
      for (const node of [payer, payee]) {
        await node.start();
        await node.waitForEngine();
      }
      await scenario.until(
        "caught up",
        () => settledOverHub(scenario),
        HUB_WITHIN_MS,
      );
      assertEquals(
        await Promise.all([payer, payee].map(hubBooks)),
        kept,
        "the books after a restart",
      );
      assertEquals(
        await peerAccounts(scenario, HUB_LEDGER),
        accounts,
        "peer accounts",
      );

      // A node added later joins the hub as a member: its scheme takes the
      // founded hub's rules, and it funds its account from its owner book,
      // then peers over the hub alone.
      const added = await scenario.addNode();
      const { activeSettlementSchemes } = await added.rpc.call(
        "getBasicState",
        {},
      );
      assert(
        activeSettlementSchemes.includes(HUB_SCHEME),
        `${added.name} runs ${activeSettlementSchemes.join(", ")}`,
      );
      await scenario.until(
        `${added.name} funded in the hub`,
        () => fundedInHub(added),
        HUB_WITHIN_MS,
      );
      await scenario.until(
        `${added.name} peered over the hub`,
        async () =>
          (await peersOverHub(added, payer)) ||
          (await peersOverHub(added, payee)),
        HUB_WITHIN_MS,
      );
      await assertPeersOverHubAlone(added);
    } finally {
      const failures = await scenario.stop();
      assertEquals(failures, [], failures.join("\n"));
    }
  },
);
