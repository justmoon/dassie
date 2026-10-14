/**
 * Milestone M5's other done criterion (spec 7 §8): `deno task dev` brings
 * up any scenario, and its dashboard shows it (spec 6 §2). The dev server
 * runs as a developer would run it, headless and moved to other ports, on
 * the release-like binary, with a CA of its own; Chrome drives the
 * dashboard through the SNI proxy, and the nodes are asked over IPC.
 * `DASSIE_E2E_SCREENSHOTS` names a directory for a picture of each step.
 */

import { assert, assertEquals, assertMatch } from "@std/assert";
import type { Page } from "@astral/astral";
import { RpcClient } from "@dassie/rpc";
import { ipcConnector } from "@dassie/rpc/ipc";
import { ChildProcess, type Exit } from "@dassie/dev/process";
import { layout, socketPath } from "@dassie/dev/scenario";
import { attribute, click, count, text, until } from "./browser.ts";
import { DASSIE, missingTools, startBrowser } from "./harness.ts";

const missing = await missingTools();
if (missing) console.warn(`skipping the dev environment tests: ${missing}`);

const ROOT = new URL("../../", import.meta.url).pathname;
/** Away from the dev environment's own ports and the other tests'. */
const OFFSET = 16_000;
const PROXY = 443 + OFFSET;
const DASHBOARD = 10_100 + OFFSET;
const httpsPort = (index: number) => 5_001 + index + OFFSET;

/** The dev server, as `deno task dev` starts it, with test flags. */
class DevServer {
  readonly child: ChildProcess;
  readonly #lines: string[] = [];

  constructor(state: string, caRoot: string, more: string[] = []) {
    this.child = new ChildProcess("dev", Deno.execPath(), {
      args: [
        "run",
        "-A",
        `${ROOT}apps/dev/main.ts`,
        "--headless",
        "--no-watch",
        "--dassie",
        DASSIE,
        "--port-offset",
        `${OFFSET}`,
        "--state-dir",
        state,
        ...more,
      ],
      env: { CAROOT: caRoot },
      onLine: (_, line) => this.#lines.push(line),
    });
  }

  /** Waits until the dev server says `pattern`, `times` times in all. */
  async said(pattern: RegExp, times = 1, timeoutMs = 180_000): Promise<void> {
    const deadline = Date.now() + timeoutMs;
    let exited: Exit | undefined;
    this.child.exited.then((exit) => exited = exit);
    while (this.#lines.filter((line) => pattern.test(line)).length < times) {
      if (exited) {
        throw new Error(
          `the dev server exited ${exited.code}:\n${exited.output}`,
        );
      }
      if (Date.now() > deadline) {
        throw new Error(
          `the dev server did not say ${pattern}:\n${this.output}`,
        );
      }
      await new Promise((resolve) => setTimeout(resolve, 200));
    }
  }

  get output(): string {
    return this.#lines.join("\n");
  }
}

/** The owner's client of a dev node, over its IPC socket. */
async function owner(state: string, name: string): Promise<RpcClient> {
  return new RpcClient({
    url: "ipc",
    connect: ipcConnector(socketPath(await layout(state), name)),
  });
}

/** Whether nothing listens on `port` any more. */
async function closed(port: number): Promise<boolean> {
  try {
    (await Deno.connect({ hostname: "127.0.0.1", port })).close();
    return false;
  } catch {
    return true;
  }
}

const screenshots = Deno.env.get("DASSIE_E2E_SCREENSHOTS");

/** Keeps a picture of the page, if pictures are asked for. */
async function picture(page: Page, name: string): Promise<void> {
  if (!screenshots) return;
  await Deno.mkdir(screenshots, { recursive: true });
  await Deno.writeFile(`${screenshots}/${name}.png`, await page.screenshot());
}

/** How many nodes the sidebar shows running. */
function running(page: Page): Promise<number> {
  return page.evaluate(() =>
    document.querySelectorAll('[data-testid^="node-d"][data-state="running"]')
      .length
  );
}

Deno.test({
  name:
    "deno task dev brings up a scenario, and its dashboard shows and drives it",
  ignore: missing !== undefined,
  sanitizeResources: false,
  sanitizeOps: false,
  async fn(t) {
    const state = await Deno.makeTempDir({ prefix: "dassie-dev-" });
    const caRoot = await Deno.makeTempDir({ prefix: "dassie-dev-ca-" });
    const browserDir = await Deno.makeTempDir({ prefix: "dassie-dev-chrome-" });
    let dev = new DevServer(state, caRoot);
    const browser = await startBrowser(browserDir);
    try {
      await dev.said(/two-nodes is running/);
      const page = await browser.newPage();
      const errors: string[] = [];
      page.addEventListener("pageerror", (event) => {
        errors.push(String(event.detail));
      });

      await t.step(
        "the dashboard shows the network through the proxy",
        async () => {
          await page.goto(`https://localhost:${PROXY}/`, { waitUntil: "load" });
          await until(
            page,
            "both nodes running",
            () => running(page),
            (n) => n === 2,
          );
          await until(
            page,
            "two nodes drawn",
            () => count(page, "graph-node"),
            (n) => n === 2,
          );
          await until(
            page,
            "they peer",
            () => count(page, "graph-link"),
            (n) => n === 1,
          );
          await until(
            page,
            "log lines",
            () => count(page, "log-row"),
            (n) => n > 0,
          );
          await picture(page, "dev-01-dashboard");
        },
      );

      await t.step(
        "the control channel and the relay refuse other pages",
        async () => {
          const ca = await Deno.readTextFile(`${caRoot}/rootCA.pem`);
          const client = Deno.createHttpClient({ caCerts: [ca], http2: false });
          const upgrade = (path: string, origin?: string) =>
            fetch(`https://localhost:${DASHBOARD}${path}`, {
              client,
              headers: {
                ...(origin ? { origin } : {}),
                upgrade: "websocket",
                connection: "upgrade",
                "sec-websocket-key": "dGhlIHNhbXBsZSBub25jZQ==",
                "sec-websocket-version": "13",
                "sec-websocket-protocol": "dassie.ui.v1",
              },
            }).then(async (response) => {
              await response.body?.cancel();
              return response.status;
            });
          const own = `https://localhost:${DASHBOARD}`;
          assertEquals(
            await upgrade("/nodes/d1/rpc", "https://evil.example"),
            403,
          );
          assertEquals(await upgrade("/control"), 403);
          assertEquals(await upgrade("/nodes/d9/rpc", own), 404);
          client.close();
        },
      );

      await t.step("a node's debug pages work through the relay", async () => {
        await page.goto(`https://localhost:${PROXY}/nodes/d1/debug/database`, {
          waitUntil: "load",
        });
        await click(page, "db-table-node_config");
        await until(page, "a row", () => count(page, "db-row"), (n) => n === 1);
        await picture(page, "dev-02-node-database");
        await page.goto(`https://localhost:${PROXY}/nodes/host`, {
          waitUntil: "load",
        });
        await text(page, "page-host");
        await picture(page, "dev-03-host");
      });

      await t.step("the wallet link opens the wallet signed in", async () => {
        await page.goto(`https://localhost:${PROXY}/`, { waitUntil: "load" });
        const href = await attribute(page, "wallet-d1", "href");
        assertMatch(
          href ?? "",
          /^https:\/\/d1\.localhost:16443\/#dassie-dev-login=[0-9a-f]{64}$/,
        );
        const wallet = await browser.newPage();
        await wallet.goto(href!, { waitUntil: "load" });
        assertEquals(await text(wallet, "payment-pointer"), "$d1.localhost");
        assertEquals(await wallet.evaluate(() => location.hash), "");
        await picture(wallet, "dev-04-wallet");
        await wallet.close();
      });

      await t.step("a node added joins the network", async () => {
        await page.goto(`https://localhost:${PROXY}/`, { waitUntil: "load" });
        await until(page, "connected", () => running(page), (n) => n === 2);
        await click(page, "add-node");
        await until(
          page,
          "d3 running",
          () => running(page),
          (n) => n === 3,
          90_000,
        );
        await until(
          page,
          "d3 peers",
          () => count(page, "graph-link"),
          (n) => n >= 2,
          90_000,
        );
      });

      await t.step("another scenario starts from scratch", async () => {
        await page.goto(`https://localhost:${PROXY}/scenarios`, {
          waitUntil: "load",
        });
        await picture(page, "dev-05-scenarios");
        await click(page, "run-six-nodes");
        await dev.said(/six-nodes is running/);
        await until(
          page,
          "six running",
          () => running(page),
          (n) => n === 6,
          60_000,
        );
        await until(
          page,
          "they peer",
          () => count(page, "graph-link"),
          (n) => n >= 6,
          120_000,
        );
        await picture(page, "dev-06-six-nodes");
      });

      await t.step("a restart resumes, without setting up again", async () => {
        const before = await owner(state, "d1");
        await before.call("stubDeposit", { amount: 7n });
        const [ledger] = (await before.call("listLedgers", {})).ledgers;
        before.close();
        const exit = await dev.child.stop();
        assertEquals(exit.code, 0, exit.output);
        for (let index = 0; index < 6; index++) {
          assert(await closed(httpsPort(index)), `d${index + 1} still runs`);
        }
        dev = new DevServer(state, caRoot);
        await dev.said(/six-nodes is running/);
        assert(!/from scratch/.test(dev.output), dev.output);
        const after = await owner(state, "d1");
        const [again] = (await after.call("listLedgers", {})).ledgers;
        after.close();
        assertEquals(again.ownerBalance, ledger.ownerBalance);
      });

      await t.step(
        "a second dev server on the same state stops at once",
        async () => {
          const second = new DevServer(state, caRoot);
          const exit = await second.child.exited;
          assertEquals(exit.code, 1);
          assertMatch(exit.output, /another dev environment \(PID \d+\) runs/);
          // The first one runs on.
          const d1 = await owner(state, "d1");
          await d1.call("listLedgers", {});
          d1.close();
        },
      );

      await t.step(
        "naming another scenario starts it from scratch",
        async () => {
          const exit = await dev.child.stop();
          assertEquals(exit.code, 0, exit.output);
          dev = new DevServer(state, caRoot, ["--scenario", "two-nodes"]);
          await dev.said(/two-nodes is running/);
          assertMatch(dev.output, /starting two-nodes from scratch/);
          assert(await closed(httpsPort(2)), "d3 of six-nodes still runs");
        },
      );

      assertEquals(errors, [], "the dashboard threw");
      await page.close();
    } finally {
      await browser.close();
      const exit = await dev.child.stop();
      for (let index = 0; index < 6; index++) {
        assert(await closed(httpsPort(index)), `d${index + 1} still runs`);
      }
      const { sockets } = await layout(state);
      for (const dir of [state, caRoot, browserDir, sockets]) {
        await Deno.remove(dir, { recursive: true }).catch(() => {});
      }
      assertEquals(exit.code, 0, exit.output);
    }
  },
});
