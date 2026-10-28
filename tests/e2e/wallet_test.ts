// End to end (spec/areas/testing.md#end-to-end): every page of the wallet
// (spec/areas/wallet.md#pages) works against real nodes, and two nodes pay
// each other through Open Payments, from the wallet's Send. A `two-nodes`
// scenario runs, and a headless browser signed in to `d1` goes through the
// pages; live data must arrive without a reload. A `hub` scenario then
// shows the Hubs page on a member of a settlement hub, and a deposit made
// there. `DASSIE_E2E_WALLET_DIR` serves a wallet build other than the one
// embedded in the binary, for working on the wallet, and
// `DASSIE_E2E_SCREENSHOTS` names a directory for a picture of each step.

import { test } from "node:test";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";

import { assert, assertEquals, assertMatch } from "@std/assert";
import type { Page } from "puppeteer-core";
import {
  attribute,
  click,
  count,
  signIn,
  text,
  type,
  until,
} from "./browser.ts";
import {
  DASSIE,
  FAKE_LEDGER,
  missingScenarioTools,
  missingTools,
  PORT_OFFSETS,
  startBrowser,
} from "./harness.ts";
import { HUB_SCHEME, Scenario, type ScenarioNode } from "@dassie/dev/scenario";

/**
 * The tests here run one after another, each scenario stopped before the
 * next starts, so they share one offset, clear of the other files' ports
 * (`PORT_OFFSETS`, which ports_test.ts checks).
 */
const PORT_OFFSET = PORT_OFFSETS["wallet_test.ts"];

const missing = await missingTools();
if (missing) console.warn(`skipping the wallet tests: ${missing}`);
const missingHub = missing ?? await missingScenarioTools();

/**
 * How long the hub's members may take to open their accounts and peer over
 * it, and a deposit to reach it: each move and operation waits for a
 * validated ledger and a final block, as in hub_test.ts.
 */
const HUB_WITHIN_MS = 180_000;

const devWallet = process.env.DASSIE_E2E_WALLET_DIR;
const screenshots = process.env.DASSIE_E2E_SCREENSHOTS;
const env = devWallet ? { DASSIE_DEV_WALLET_DIR: devWallet } : undefined;

/** Signs the browser in to `node` as its owner, and opens the wallet. */
async function signInWallet(page: Page, node: ScenarioNode): Promise<void> {
  await signIn(page, node);
  await page.goto(node.url, { waitUntil: "load" });
}

/** Goes to `path` of the wallet the way a click on a link would. */
async function visit(page: Page, node: ScenarioNode, path: string) {
  await page.goto(`${node.url}${path}`, { waitUntil: "load" });
}

/** Keeps a picture of the page, if pictures are asked for. */
async function picture(page: Page, name: string): Promise<void> {
  if (!screenshots) return;
  await mkdir(screenshots, { recursive: true });
  await writeFile(`${screenshots}/${name}.png`, await page.screenshot());
}

/** Fails on any error the page throws. */
function watchErrors(page: Page): string[] {
  const errors: string[] = [];
  page.on("pageerror", (error) => {
    errors.push(String(error));
  });
  return errors;
}

test("every page of the wallet works against two nodes", {
  skip: missing !== undefined,
}, async (t) => {
  const scenario = await Scenario.temporary("two-nodes", {
    dassie: DASSIE,
    portOffset: PORT_OFFSET,
    env,
  });
  const dir = await mkdtemp(join(tmpdir(), "dassie-wallet-"));
  const browser = await startBrowser(dir);
  try {
    await scenario.until("the nodes peer", () => scenario.converged());
    const [d1, d2] = scenario.nodes;
    const page = await browser.newPage();
    const errors = watchErrors(page);
    await signInWallet(page, d1);

    await t.test("the dashboard follows the balance live", async () => {
      await until(
        page,
        "the default wallet address",
        () => text(page, "payment-pointer"),
        (pointer) => pointer === `$${d1.hostname}:${d1.httpsPort}`,
      );
      assertEquals(
        await attribute(page, "balance-usd", "data-value"),
        "100000000000",
      );
      await until(
        page,
        "both nodes counted",
        () => text(page, "node-count"),
        (
          n,
        ) => n === "2 nodes",
      );
      assertEquals(
        await attribute(page, "network-status", "class"),
        "status",
      );
      // A deposit made elsewhere shows without a reload.
      await d1.rpc.call("stubDeposit", { amount: 5_000_000_000n });
      await until(
        page,
        "the deposit shows",
        () => attribute(page, "balance-usd", "data-value"),
        (value) => value === "105000000000",
      );
      await picture(page, "01-dashboard");
    });

    await t.test(
      "receive: the stub's test money, and a request for a payment",
      async () => {
        await click(page, "receive-button");
        await click(page, "tab-stub");
        await click(page, "stub-deposit");
        await until(
          page,
          "the $100 arrives",
          () => attribute(page, "balance-usd", "data-value"),
          (value) => value === "205000000000",
        );
        await click(page, "receive-button");
        assertEquals(
          await text(page, "receive-pointer"),
          `$${d1.hostname}:${d1.httpsPort}`,
        );
        await type(page, "request-amount", "1.5");
        await type(page, "request-description", "e2e request");
        await click(page, "request-create");
        const url = await text(page, "request-url");
        assertMatch(url, /\/incoming-payments\/[0-9a-f-]{36}$/);
        await click(page, "stream-credentials");
        assertMatch(await text(page, "stream-destination"), /^test\.das\./);
        // A change of balance does not take the request away.
        await d1.rpc.call("stubDeposit", { amount: 1n });
        await new Promise((resolve) => setTimeout(resolve, 1_500));
        assertEquals(await count(page, "stream-destination"), 1);
        await picture(page, "02-receive");
        // Activity shows it, and completes it.
        await click(page, "request-activity");
        const id = url.split("/").at(-1);
        await until(
          page,
          "the request is listed",
          () => attribute(page, `incoming-${id}`, "data-state"),
          (state) => state === "open",
        );
        await click(page, "incoming-complete");
        await click(page, "incoming-complete-confirm");
        await until(
          page,
          "the request is completed",
          () => attribute(page, `incoming-${id}`, "data-state"),
          (state) => state === "completed",
        );
        await picture(page, "02-activity");
      },
    );

    await t.test(
      "send: to d2's payment pointer, and its status",
      async () => {
        const balance = async (node: ScenarioNode) =>
          (await node.rpc.call("listLedgers", {})).ledgers[0].ownerBalance;
        const [before1, before2] = [await balance(d1), await balance(d2)];
        await visit(page, d1, "/");
        await click(page, "send-button");
        const pointer = `$${d2.hostname}:${d2.httpsPort}`;
        await type(page, "send-recipient", pointer);
        await until(
          page,
          "d2's wallet address is found",
          () => text(page, "send-recipient-name"),
          (name) => name.includes(pointer),
        );
        await click(page, "send-continue");
        await type(page, "send-amount", "1.5");
        await type(page, "send-note", "e2e send");
        await click(page, "send-review");
        // What a probe measured, less STREAM's 1% slippage.
        assertEquals(
          await attribute(page, "send-debit", "data-value"),
          "1515151516",
        );
        assertEquals(
          await attribute(page, "send-receive", "data-value"),
          "1500000000",
        );
        await picture(page, "03-send-review");
        await click(page, "send-confirm");
        await until(
          page,
          "the payment completes",
          () => attribute(page, "payment", "data-state"),
          (state) => state === "completed",
        );
        // It fixed what d2 gets, so it leads with that.
        assertEquals(
          await attribute(page, "payment-receive", "data-value"),
          "1500000000",
        );
        assertEquals(
          await attribute(page, "payment-sent", "data-value"),
          "1500000000",
        );
        assertEquals(await text(page, "payment-recipient"), pointer);
        await picture(page, "03-payment");
        // d2 has it, with the note; the money moved. d2 counted each
        // packet received as it posted it, before its Fulfill went back
        // (spec/decisions/own-fulfills.md).
        const sent = async () => {
          const { payments } = await d2.rpc.call("listIncomingPayments", {});
          return payments.find((payment) =>
            payment.metadataJson.includes("e2e send")
          );
        };
        await scenario.until(
          "d2 has it all",
          async () => (await sent())?.receivedAmount === 1_500_000_000n,
        );
        const received = await sent();
        assertEquals(received?.receivedAmount, 1_500_000_000n);
        assertEquals(received?.paidInFull, true);
        assertEquals(await balance(d1), before1 - 1_500_000_000n);
        await scenario.until(
          "d2's balance moves",
          async () => (await balance(d2)) === before2 + 1_500_000_000n,
        );
        // Activity leads back to it.
        const id = await page.evaluate(() =>
          location.pathname.split("/").at(-1)
        );
        await click(page, "payment-activity");
        await until(
          page,
          "the payment is listed",
          () => attribute(page, `outgoing-${id}`, "data-state"),
          (state) => state === "completed",
        );
        await click(page, "outgoing-link");
        await until(
          page,
          "its page again",
          () => attribute(page, "payment", "data-state"),
          (state) => state === "completed",
        );
      },
    );

    await t.test("send: pay d2's payment request", async () => {
      const { payment } = await d2.rpc.call("createIncomingPayment", {
        incomingAmount: 250_000_000n,
        description: "e2e invoice",
      });
      assert(payment);
      await visit(page, d1, `/send?to=${encodeURIComponent(payment.url)}`);
      await until(
        page,
        "the request is read",
        () => text(page, "send-request"),
        (request) => request.includes("e2e invoice"),
      );
      await click(page, "send-continue");
      assertEquals(
        await page.evaluate(() =>
          (document.querySelector(
            '[data-testid="send-amount"]',
          ) as HTMLInputElement).value
        ),
        "0.25",
      );
      await click(page, "send-review");
      await click(page, "send-confirm");
      await until(
        page,
        "the request is paid",
        () => attribute(page, "payment", "data-state"),
        (state) => state === "completed",
      );
      // d2 counted it received as it posted it
      // (spec/decisions/own-fulfills.md).
      const paid = async () => {
        const { payments } = await d2.rpc.call("listIncomingPayments", {});
        return payments.find((entry) => entry.id === payment.id)
          ?.receivedAmount;
      };
      await scenario.until(
        "d2 has it all",
        async () => (await paid()) === 250_000_000n,
      );
      assertEquals(await paid(), 250_000_000n);
    });

    // Its removal would settle up with the peer first, and then drop its
    // method (spec/decisions/settlement-methods.md): peers no longer keep
    // it. The page says so; the ledger stays, for the steps after.
    await t.test("ledgers: peers do not keep a ledger", async () => {
      await visit(page, d1, "/ledgers");
      await text(page, "ledger-stub+usd");
      await picture(page, "03-ledgers");
      await click(page, "ledger-delete-stub");
      assertEquals(
        await attribute(page, "prereq-peers", "data-state"),
        "valid",
      );
    });

    await t.test("hubs: a node in no hub says how to join one", async () => {
      await click(page, "nav-hubs");
      await text(page, "hubs-none");
    });

    await t.test(
      "network: the graph, the listing and a peer's details",
      async () => {
        await visit(page, d1, "/network");
        await until(
          page,
          "two nodes drawn",
          () => count(page, "graph-node"),
          (
            n,
          ) => n === 2,
        );
        assertEquals(await count(page, "graph-link"), 1);
        await click(page, `node-row-${d2.id}`);
        assertEquals(await text(page, "peer-state"), "peered");
        assertEquals(await text(page, "node-url"), d2.url);
        await picture(page, "04-network");
      },
    );

    await t.test("register: renewing now", async () => {
      await visit(page, d1, "/network/register");
      await click(page, `renew-${d2.id}`);
      assertEquals(await text(page, "toast-title"), "Registration sent");
      await picture(page, "05-register");
    });

    await t.test(
      "settings: alias, sessions, and a capped token",
      async () => {
        await visit(page, d1, "/settings");
        assertEquals(await text(page, "settings-node-id"), d1.id);
        assertEquals(
          await text(page, "settings-version"),
          (await d1.rpc.call("getStatus", {})).version,
        );
        await text(page, "session-current");
        // The browser is on loopback, so it may pair a device.
        await text(page, "devices-none");
        await click(page, "pair-device");
        assertEquals(
          await attribute(page, "pairing-qr", "aria-label"),
          "Pairing code",
        );
        await type(page, "alias-input", "alpha");
        await click(page, "alias-save");
        // The peer hears of it through the link state.
        await scenario.until("d2 sees the alias", async () => {
          const { nodes } = await d2.rpc.call("listNodes", {});
          return nodes.some((node) =>
            node.id === d1.id && node.alias === "alpha"
          );
        });
        await type(page, "token-label", "shop");
        await type(page, "token-cap-amount", "1");
        await click(page, "create-token");
        await click(page, "new-token-url-reveal");
        assertMatch(await text(page, "new-token-url"), /^btp\+wss:\/\/:/);
        await picture(page, "06-new-token");
        await click(page, "new-token-close");
        const { tokens } = await d1.rpc.call("listBtpTokens", {});
        assertEquals(tokens[0].cap?.amount, 1_000_000_000n);
        assertMatch(
          await text(page, `btp-token-cap-${tokens[0].id}`),
          /\$ 1\.00 per day/,
        );
        await picture(page, "07-settings");
        // The cap editor starts from the cap; a new window keeps the
        // amount.
        await click(page, `edit-cap-${tokens[0].id}`);
        const editor = await page.evaluate(() => {
          const form = document.querySelector('[data-testid="cap-editor"]')!;
          const window = form.querySelector<HTMLSelectElement>(
            '[data-testid="token-cap-window"]',
          )!;
          window.value = "week";
          window.dispatchEvent(new Event("change", { bubbles: true }));
          return form.querySelector<HTMLInputElement>(
            '[data-testid="token-cap-amount"]',
          )!.value;
        });
        assertEquals(editor, "1");
        await click(page, "save-cap");
        await until(
          page,
          "the new window",
          () => text(page, `btp-token-cap-${tokens[0].id}`),
          (cap) => /\$ 1\.00 per week/.test(cap),
        );
      },
    );

    await t.test("the theme is kept across a reload", async () => {
      await click(page, "theme-dark");
      await page.reload({ waitUntil: "load" });
      await text(page, "nav-settings");
      assertEquals(
        await page.evaluate(() => document.documentElement.dataset.theme),
        "dark",
      );
      await picture(page, "08-dark");
    });

    await t.test("debug pages show the node", async () => {
      await visit(page, d1, "/debug/logs");
      await until(
        page,
        "log lines",
        () => count(page, "log-row"),
        (n) => n > 0,
      );
      await picture(page, "09-logs");
      await visit(page, d1, "/debug/nodes");
      await text(page, `debug-node-${d2.id}`);
      await visit(page, d1, "/debug/ledger");
      await click(page, "account-link-stub+usd:equity/owner");
      await until(
        page,
        "transfers",
        () => count(page, "transfer-row"),
        (n) => n > 0,
      );
      await picture(page, "10-account");
      // Another account's link shows that account.
      await click(page, "counter-link");
      await until(
        page,
        "the other account",
        () => text(page, "account-path"),
        (path) => path === "assets/settlement",
      );
      await visit(page, d1, "/debug/routing");
      await until(
        page,
        "a route",
        () => count(page, "route-row"),
        (n) => n === 1,
      );
      await picture(page, "11-routing");
      await visit(page, d1, "/debug/state");
      await text(page, "service-engine");
      await picture(page, "12-state");
      await visit(page, d1, "/debug/database");
      await click(page, "db-table-node_config");
      await until(page, "a row", () => count(page, "db-row"), (n) => n === 1);
      await picture(page, "13-database");
      await visit(page, d1, "/debug/packets");
      await text(page, "debug-packets");
    });

    await t.test("ending a peering, and a ban", async () => {
      await visit(page, d1, `/network/${d2.id}`);
      await click(page, "terminate-peering");
      await click(page, "terminate-peering-confirm");
      await until(page, "terminating", () => text(page, "peer-state"), (
        state,
      ) => state === "terminating" || state === "none");
      await click(page, "ban-node");
      await click(page, "ban-node-confirm");
      await click(page, "unban-node");
      await until(
        page,
        "unbanned",
        () => count(page, "ban-node"),
        (n) => n === 1,
      );
    });

    assertEquals(errors, [], "the page threw");
    await page.close();
  } finally {
    await browser.close();
    const failures = await scenario.stop();
    await rm(dir, { recursive: true, force: true }).catch(() => {});
    assertEquals(failures, []);
  }
});

test(
  "a node without a ledger asks for one, and a removed last ledger asks again",
  { skip: missing !== undefined },
  async () => {
    const scenario = await Scenario.temporary("one-node", {
      dassie: DASSIE,
      portOffset: PORT_OFFSET,
      env,
      withoutLedger: [0],
    });
    const dir = await mkdtemp(join(tmpdir(), "dassie-wallet-"));
    const browser = await startBrowser(dir);
    try {
      const [node] = scenario.nodes;
      const page = await browser.newPage();
      const errors = watchErrors(page);
      await signInWallet(page, node);
      assertEquals(await text(page, "node-id"), node.id);
      await text(page, "testnet-alert");
      await click(page, "select-scheme-stub");
      // The wallet, at once, without a reload.
      await text(page, "ledger-stub+usd");
      await click(page, "ledger-delete-stub");
      assertEquals(
        await attribute(page, "prereq-peers", "data-state"),
        "valid",
      );
      await type(page, "delete-confirmation", "DELETE STUB");
      await click(page, "delete-submit");
      await text(page, "page-create-ledger");
      assert(
        !(await page.evaluate(() => location.pathname.startsWith("/setup"))),
      );
      assertEquals(errors, [], "the page threw");
      await page.close();
    } finally {
      await browser.close();
      const failures = await scenario.stop();
      await rm(dir, { recursive: true, force: true }).catch(() => {});
      assertEquals(failures, []);
    }
  },
);

/** Whether `node` peers with `peer` over the hub. */
async function peersOverHub(
  node: ScenarioNode,
  peer: ScenarioNode,
): Promise<boolean> {
  const { peers } = await node.rpc.call("listPeers", {});
  return peers.some((p) =>
    p.node === peer.id && p.state === "peered" &&
    p.methods.some((m) => m.scheme === HUB_SCHEME)
  );
}

/** `node`'s one hub, as the node reports it. */
async function hubOf(node: ScenarioNode) {
  const { hubs } = await node.rpc.call("getHubStatus", {});
  assertEquals(hubs.length, 1, `${node.name}'s hubs`);
  return hubs[0];
}

// The Hubs page (spec/areas/wallet.md#pages) on a member of the `hub`
// scenario, once the members peered over the hub, as hub_test.ts waits for
// it: it shows the hub with its balance and the holds toward the other
// member, as the node reports them, a deposit made on the page reaches the
// hub, and the page turns the node's own moves, off for the deposit, on
// again.
test(
  "the hubs page shows a member's hub, and a deposit made on it reaches the hub",
  { skip: missingHub !== undefined },
  async () => {
    const scenario = await Scenario.temporary("hub", {
      dassie: DASSIE,
      fakeLedger: FAKE_LEDGER,
      portOffset: PORT_OFFSET,
      env,
    });
    const dir = await mkdtemp(join(tmpdir(), "dassie-wallet-"));
    const browser = await startBrowser(dir);
    try {
      const members = scenario.def.hub?.members ?? [];
      const [member, peer] = scenario.nodes.filter((node) =>
        members.includes(node.index)
      );
      assert(member && peer, "the scenario has two members");
      await scenario.until(
        "peered over the hub",
        async () =>
          (await peersOverHub(member, peer)) &&
          (await peersOverHub(peer, member)),
        HUB_WITHIN_MS,
      );
      // The member holds something for its peer.
      await scenario.until("a hold funded, nothing moving", async () => {
        const { hubs: [hub] } = await member.rpc.call("getHubStatus", {});
        return hub !== undefined && hub.moving === undefined &&
          hub.balance > 0n &&
          hub.paying.some((hold) => hold.peer === peer.id && hold.amount > 0n);
      }, HUB_WITHIN_MS);
      // Quiet: the node's own moves off, and none on its way, so the
      // deposit below is the only one.
      const { hubId: id, scheme } = await hubOf(member);
      await member.rpc.call("setHubAutomatic", { scheme, automatic: false });
      await scenario.until(
        "nothing moving",
        async () => (await hubOf(member)).moving === undefined,
        HUB_WITHIN_MS,
      );
      const before = await hubOf(member);

      const page = await browser.newPage();
      const errors = watchErrors(page);
      await signInWallet(page, member);
      await click(page, "nav-hubs");
      // As the node reports them: the balance, and the hold toward the
      // peer, in the hub ledger's units.
      await until(
        page,
        "the balance shows",
        async () => [
          await attribute(page, `hub-balance-${id}`, "data-value"),
          `${(await hubOf(member)).balance}`,
        ],
        ([shown, reported]) => shown === reported,
      );
      const hold = before.paying.find((hold) => hold.peer === peer.id)!;
      assertEquals(await count(page, `hub-paying-${id}-${hold.account}`), 1);
      await until(
        page,
        "the holds show",
        async () => [
          await attribute(page, `hub-paying-${id}-total`, "data-value"),
          `${
            (await hubOf(member)).paying.reduce((sum, h) => sum + h.amount, 0n)
          }`,
        ],
        ([shown, reported]) => shown === reported,
      );
      await picture(page, "10-hubs");

      // 1 XRP, at the hub ledger's scale 9.
      const deposit = 1_000_000_000n;
      await type(page, `hub-amount-${id}`, "1");
      await click(page, `hub-deposit-${id}`);
      await until(
        page,
        "the deposit reaches the hub",
        async () => [
          await attribute(page, `hub-deposited-${id}`, "data-value"),
          await count(page, `hub-error-${id}`) > 0
            ? await text(page, `hub-error-${id}`)
            : "",
        ],
        ([value, refused]) => {
          // A refusal fails here, not at the deadline.
          assertEquals(refused, "", "the node refused the deposit");
          return value === `${before.deposited + deposit}`;
        },
        HUB_WITHIN_MS,
      );
      const after = await hubOf(member);
      assertEquals(after.deposited, before.deposited + deposit);
      await picture(page, "11-hubs-deposit");
      // The node's own moves, turned on again on the page.
      assertMatch(
        await text(page, `hub-automatic-text-${id}`),
        /moves nothing by itself/,
      );
      await click(page, `hub-automatic-${id}`);
      await until(
        page,
        "the node's own moves on",
        () => text(page, `hub-automatic-text-${id}`),
        (shown) => !shown.includes("moves nothing by itself"),
      );
      assert((await hubOf(member)).automatic, "the node's own moves are on");
      assertEquals(await count(page, `hub-error-${id}`), 0);
      assertEquals(errors, [], "the page threw");
      await page.close();
    } finally {
      await browser.close();
      const failures = await scenario.stop();
      await rm(dir, { recursive: true, force: true }).catch(() => {});
      assertEquals(failures, []);
    }
  },
);
