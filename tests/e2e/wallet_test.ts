// Milestone M5a's done criterion (spec 7 §8): every page of the wallet
// (spec 6 §1.6, but Send and payment status, which come with M6) works
// against real nodes. A `two-nodes` scenario runs, and a headless browser
// signed in to `d0` goes through the pages; live data must arrive without a
// reload. `DASSIE_E2E_WALLET_DIR` serves a wallet build other than the one
// embedded in the binary, for working on the wallet, and
// `DASSIE_E2E_SCREENSHOTS` names a directory for a picture of each step.

import { assert, assertEquals, assertMatch } from "@std/assert";
import { encodeHex } from "@std/encoding/hex";
import { loginSecret } from "@dassie/identity-wasm";
import type { Page } from "@astral/astral";
import { attribute, click, count, text, type, until } from "./browser.ts";
import { missingTools, startBrowser } from "./harness.ts";
import { Scenario, type ScenarioNode } from "./scenarios.ts";

const missing = await missingTools();
if (missing) console.warn(`skipping the wallet tests: ${missing}`);

const devWallet = Deno.env.get("DASSIE_E2E_WALLET_DIR");
const screenshots = Deno.env.get("DASSIE_E2E_SCREENSHOTS");
const env = devWallet ? { DASSIE_DEV_WALLET_DIR: devWallet } : undefined;

/** Signs the browser in to `node` as its owner, with the node's key. */
async function signIn(page: Page, node: ScenarioNode): Promise<void> {
  await page.goto(node.url, { waitUntil: "load" });
  const proof = encodeHex(loginSecret(node.key));
  const status = await page.evaluate(
    async (proof: string) => {
      const response = await fetch("/api/login", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ loginAuthorizationSignature: proof }),
      });
      return response.status;
    },
    { args: [proof] },
  );
  assertEquals(status, 200);
  await page.goto(node.url, { waitUntil: "load" });
}

/** Goes to `path` of the wallet the way a click on a link would. */
async function visit(page: Page, node: ScenarioNode, path: string) {
  await page.goto(`${node.url}${path}`, { waitUntil: "load" });
}

/** Keeps a picture of the page, if pictures are asked for. */
async function picture(page: Page, name: string): Promise<void> {
  if (!screenshots) return;
  await Deno.mkdir(screenshots, { recursive: true });
  await Deno.writeFile(`${screenshots}/${name}.png`, await page.screenshot());
}

/** Fails on any error the page throws. */
function watchErrors(page: Page): string[] {
  const errors: string[] = [];
  page.addEventListener("pageerror", (event) => {
    errors.push(String(event.detail));
  });
  return errors;
}

Deno.test({
  name: "every page of the wallet works against two nodes",
  ignore: missing !== undefined,
  sanitizeResources: false,
  sanitizeOps: false,
  async fn(t) {
    const scenario = await Scenario.start("two-nodes", { env });
    const dir = await Deno.makeTempDir({ prefix: "dassie-wallet-" });
    const browser = await startBrowser(dir);
    try {
      await scenario.until("the nodes peer", () => scenario.converged());
      const [d0, d1] = scenario.nodes;
      const page = await browser.newPage();
      const errors = watchErrors(page);
      await signIn(page, d0);

      await t.step("the dashboard follows the balance live", async () => {
        assertEquals(await text(page, "payment-pointer"), "$localhost");
        assertEquals(
          await attribute(page, "balance-stub+usd", "data-value"),
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
        await d0.rpc.call("stubDeposit", { amount: 5_000_000_000n });
        await until(
          page,
          "the deposit shows",
          () => attribute(page, "balance-stub+usd", "data-value"),
          (value) => value === "105000000000",
        );
        await picture(page, "01-dashboard");
      });

      await t.step(
        "receive: the stub's test money, and STREAM credentials",
        async () => {
          await click(page, "receive-button");
          await click(page, "tab-stub");
          await click(page, "stub-deposit");
          await until(
            page,
            "the $100 arrives",
            () => attribute(page, "balance-stub+usd", "data-value"),
            (value) => value === "205000000000",
          );
          await click(page, "receive-button");
          await click(page, "stream-credentials");
          assertMatch(await text(page, "stream-destination"), /^test\.das\./);
          await picture(page, "02-receive");
        },
      );

      await t.step("ledgers: a peered ledger cannot be removed", async () => {
        await visit(page, d0, "/ledgers");
        await text(page, "ledger-stub+usd");
        await picture(page, "03-ledgers");
        await click(page, "ledger-delete-stub");
        assertEquals(
          await attribute(page, "prereq-peers", "data-state"),
          "invalid",
        );
        await type(page, "delete-confirmation", "DELETE STUB");
        assertEquals(
          await page.evaluate(() =>
            (document.querySelector(
              '[data-testid="delete-submit"]',
            ) as HTMLButtonElement).disabled
          ),
          true,
        );
      });

      await t.step(
        "network: the graph, the listing and a peer's details",
        async () => {
          await visit(page, d0, "/network");
          await until(
            page,
            "two nodes drawn",
            () => count(page, "graph-node"),
            (
              n,
            ) => n === 2,
          );
          assertEquals(await count(page, "graph-link"), 1);
          await click(page, `node-row-${d1.id}`);
          assertEquals(await text(page, "peer-state"), "peered");
          assertEquals(await text(page, "node-url"), d1.url);
          await picture(page, "04-network");
        },
      );

      await t.step("register: renewing now", async () => {
        await visit(page, d0, "/network/register");
        await click(page, `renew-${d1.id}`);
        assertEquals(await text(page, "toast-title"), "Registration sent");
        await picture(page, "05-register");
      });

      await t.step(
        "settings: alias, sessions, and a capped token",
        async () => {
          await visit(page, d0, "/settings");
          assertEquals(await text(page, "settings-node-id"), d0.id);
          await text(page, "session-current");
          await type(page, "alias-input", "alpha");
          await click(page, "alias-save");
          // The peer hears of it through the link state.
          await scenario.until("d1 sees the alias", async () => {
            const { nodes } = await d1.rpc.call("listNodes", {});
            return nodes.some((node) =>
              node.id === d0.id && node.alias === "alpha"
            );
          });
          await type(page, "token-label", "shop");
          await type(page, "token-cap-amount", "1");
          await click(page, "create-token");
          await click(page, "new-token-url-reveal");
          assertMatch(await text(page, "new-token-url"), /^btp\+wss:\/\/:/);
          await picture(page, "06-new-token");
          await click(page, "new-token-close");
          const { tokens } = await d0.rpc.call("listBtpTokens", {});
          assertEquals(tokens[0].cap?.amount, 1_000_000_000n);
          assertMatch(
            await text(page, `btp-token-cap-${tokens[0].id}`),
            /\$ 1\.00 per day/,
          );
          await picture(page, "07-settings");
        },
      );

      await t.step("the theme is kept across a reload", async () => {
        await click(page, "theme-dark");
        await page.reload({ waitUntil: "load" });
        await text(page, "nav-settings");
        assertEquals(
          await page.evaluate(() => document.documentElement.dataset.theme),
          "dark",
        );
        await picture(page, "08-dark");
      });

      await t.step("debug pages show the node", async () => {
        await visit(page, d0, "/debug/logs");
        await until(
          page,
          "log lines",
          () => count(page, "log-row"),
          (n) => n > 0,
        );
        await picture(page, "09-logs");
        await visit(page, d0, "/debug/nodes");
        await text(page, `debug-node-${d1.id}`);
        await visit(page, d0, "/debug/ledger");
        await click(page, "account-link-stub+usd:equity/owner");
        await until(
          page,
          "transfers",
          () => count(page, "transfer-row"),
          (n) => n > 0,
        );
        await picture(page, "10-account");
        await visit(page, d0, "/debug/routing");
        await until(
          page,
          "a route",
          () => count(page, "route-row"),
          (n) => n === 1,
        );
        await picture(page, "11-routing");
        await visit(page, d0, "/debug/state");
        await text(page, "service-engine");
        await picture(page, "12-state");
        await visit(page, d0, "/debug/database");
        await click(page, "db-table-node_config");
        await until(page, "a row", () => count(page, "db-row"), (n) => n === 1);
        await picture(page, "13-database");
        await visit(page, d0, "/debug/packets");
        await text(page, "debug-packets");
      });

      await t.step("ending a peering, and a ban", async () => {
        await visit(page, d0, `/network/${d1.id}`);
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
      await Deno.remove(dir, { recursive: true }).catch(() => {});
      assertEquals(failures, []);
    }
  },
});

Deno.test({
  name:
    "a node without a ledger asks for one, and a removed last ledger asks again",
  ignore: missing !== undefined,
  sanitizeResources: false,
  sanitizeOps: false,
  async fn() {
    const scenario = await Scenario.start("one-node", {
      env,
      withoutLedger: [0],
    });
    const dir = await Deno.makeTempDir({ prefix: "dassie-wallet-" });
    const browser = await startBrowser(dir);
    try {
      const [node] = scenario.nodes;
      const page = await browser.newPage();
      const errors = watchErrors(page);
      await signIn(page, node);
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
      await Deno.remove(dir, { recursive: true }).catch(() => {});
      assertEquals(failures, []);
    }
  },
});
