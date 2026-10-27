/**
 * End to end (spec/areas/testing.md#end-to-end): `node --run dev` brings up
 * any scenario, and its dashboard shows it (spec/areas/dev.md#dashboard).
 * The dev server runs as a developer would run it, headless and moved to
 * other ports, on the release-like binary, with a CA of its own; Chrome
 * drives the dashboard through the SNI proxy, and the nodes are asked over
 * IPC.
 * `DASSIE_E2E_SCREENSHOTS` names a directory for a picture of each step.
 */

import { test } from "node:test";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { request as httpsRequest } from "node:https";
import { connect } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";

import { assert, assertEquals, assertMatch } from "@std/assert";
import type { Page } from "puppeteer-core";
import { RpcClient } from "@dassie/rpc";
import { ipcConnector } from "@dassie/rpc/ipc";
import { ChildProcess, type Exit, STOP_GRACE_MS } from "@dassie/dev/process";
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
/** How soon a second dev server on the same state exits. */
const STOPS_AT_ONCE_MS = 30_000;
/**
 * How long the dev server gets to stop cleanly: its nodes stop at once,
 * each within {@linkcode STOP_GRACE_MS}, and its server within a second
 * more; the rest is room for a slow machine.
 */
const DEV_STOP_MS = 2 * STOP_GRACE_MS + 10_000;

/** The dev server, as `node --run dev` starts it, with test flags. */
class DevServer {
  readonly child: ChildProcess;
  readonly #lines: string[] = [];

  constructor(state: string, caRoot: string, more: string[] = []) {
    this.child = new ChildProcess("dev", process.execPath, {
      args: [
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

  /** How it exited, within `timeoutMs`; stopped and thrown at otherwise. */
  async exitedWithin(timeoutMs: number): Promise<Exit> {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const late = new Promise<"late">((resolve) => {
      timer = setTimeout(() => resolve("late"), timeoutMs);
    });
    const first = await Promise.race([this.child.exited, late]);
    clearTimeout(timer);
    if (first !== "late") return first;
    const exit = await this.child.stop();
    throw new Error(
      `the dev server ran on for ${timeoutMs} ms:\n${exit.output}`,
    );
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
function closed(port: number): Promise<boolean> {
  return new Promise((resolve) => {
    const socket = connect({ host: "127.0.0.1", port });
    socket.once("connect", () => {
      socket.destroy();
      resolve(false);
    });
    socket.once("error", () => resolve(true));
  });
}

/**
 * The status a WebSocket handshake to `url` gets, over HTTP/1.1 and
 * trusting `ca`. With `node:https`, since `fetch` refuses an `upgrade`
 * header.
 */
function upgradeStatus(
  url: string,
  ca: string,
  origin?: string,
): Promise<number> {
  return new Promise((resolve, reject) => {
    const request = httpsRequest(url, {
      ca,
      agent: false,
      headers: {
        ...(origin ? { origin } : {}),
        upgrade: "websocket",
        connection: "upgrade",
        "sec-websocket-key": "dGhlIHNhbXBsZSBub25jZQ==",
        "sec-websocket-version": "13",
        "sec-websocket-protocol": "dassie.owner.v1",
      },
    });
    request.once("response", (response) => {
      resolve(response.statusCode!);
      request.destroy();
    });
    request.once("upgrade", (response, socket) => {
      resolve(response.statusCode!);
      socket.destroy();
    });
    request.once("error", reject);
    request.end();
  });
}

const screenshots = process.env.DASSIE_E2E_SCREENSHOTS;

/** Keeps a picture of the page, if pictures are asked for. */
async function picture(page: Page, name: string): Promise<void> {
  if (!screenshots) return;
  await mkdir(screenshots, { recursive: true });
  await writeFile(`${screenshots}/${name}.png`, await page.screenshot());
}

/** How many nodes the sidebar shows running. */
function running(page: Page): Promise<number> {
  return page.evaluate(() =>
    document.querySelectorAll('[data-testid^="node-d"][data-state="running"]')
      .length
  );
}

test(
  "node --run dev brings up a scenario, and its dashboard shows and drives it",
  { skip: missing !== undefined },
  async (t) => {
    const state = await mkdtemp(join(tmpdir(), "dassie-dev-"));
    const caRoot = await mkdtemp(join(tmpdir(), "dassie-dev-ca-"));
    const browserDir = await mkdtemp(join(tmpdir(), "dassie-dev-chrome-"));
    let dev = new DevServer(state, caRoot);
    const browser = await startBrowser(browserDir);
    try {
      await dev.said(/two-nodes is running/);
      const page = await browser.newPage();
      const errors: string[] = [];
      page.on("pageerror", (error) => {
        errors.push(String(error));
      });

      await t.test(
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

      await t.test(
        "the control channel and the relay refuse other pages",
        async () => {
          const ca = await readFile(`${caRoot}/rootCA.pem`, "utf8");
          const upgrade = (path: string, origin?: string) =>
            upgradeStatus(`https://localhost:${DASHBOARD}${path}`, ca, origin);
          const own = `https://localhost:${DASHBOARD}`;
          assertEquals(
            await upgrade("/nodes/d1/rpc", "https://evil.example"),
            403,
          );
          assertEquals(await upgrade("/control"), 403);
          assertEquals(await upgrade("/nodes/d9/rpc", own), 404);
        },
      );

      await t.test("a node's debug pages work through the relay", async () => {
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

      await t.test("the wallet link opens the wallet signed in", async () => {
        await page.goto(`https://localhost:${PROXY}/`, { waitUntil: "load" });
        const href = await attribute(page, "wallet-d1", "href");
        assertMatch(
          href ?? "",
          /^https:\/\/d1\.localhost:16443\/#dassie-dev-login=[0-9a-f]{64}$/,
        );
        const wallet = await browser.newPage();
        await wallet.goto(href!, { waitUntil: "load" });
        // The node's default wallet address, at its own port.
        await until(
          wallet,
          "the payment pointer",
          () => text(wallet, "payment-pointer"),
          (pointer) => pointer === `$d1.localhost:${httpsPort(0)}`,
        );
        assertEquals(await wallet.evaluate(() => location.hash), "");
        await picture(wallet, "dev-04-wallet");
        await wallet.close();
      });

      await t.test("a node added joins the network", async () => {
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

      await t.test("another scenario starts from scratch", async () => {
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

      await t.test("a restart resumes, without setting up again", async () => {
        const before = await owner(state, "d1");
        await before.call("stubDeposit", { amount: 7n });
        const [ledger] = (await before.call("listLedgers", {})).ledgers;
        before.close();
        const exit = await dev.child.stop(DEV_STOP_MS);
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

      await t.test(
        "a second dev server on the same state stops at once",
        async () => {
          const second = new DevServer(state, caRoot);
          // Bounded: if the first one is gone, the second runs on.
          const exit = await second.exitedWithin(STOPS_AT_ONCE_MS);
          assertEquals(exit.code, 1);
          assertMatch(exit.output, /another dev environment \(PID \d+\) runs/);
          // The first one runs on.
          const d1 = await owner(state, "d1");
          await d1.call("listLedgers", {});
          d1.close();
        },
      );

      await t.test(
        "naming another scenario starts it from scratch",
        async () => {
          const exit = await dev.child.stop(DEV_STOP_MS);
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
      const exit = await dev.child.stop(DEV_STOP_MS);
      for (let index = 0; index < 6; index++) {
        assert(await closed(httpsPort(index)), `d${index + 1} still runs`);
      }
      const { sockets } = await layout(state);
      for (const dir of [state, caRoot, browserDir, sockets]) {
        await rm(dir, { recursive: true, force: true }).catch(() => {});
      }
      assertEquals(exit.code, 0, exit.output);
    }
  },
);
