// End to end (spec/areas/testing.md#end-to-end): a fresh install reaches
// "setup complete" in a browser with a certificate from an ACME CA.
//
// The CA is Pebble, Let's Encrypt's test server, which runs the same ACME
// protocol and http-01 validation as Let's Encrypt's staging service; only
// the directory URL and the root differ. The daemon runs as systemd would
// run it, socket-activated, with a fresh state directory.

import { test } from "node:test";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { connect } from "node:tls";

import { assert, assertEquals, assertMatch } from "@std/assert";
import { Agent, fetch } from "undici";
import {
  deriveNodeKey,
  generateMnemonic,
  nodeId,
  publicKey,
} from "@dassie/wasm";
import type { Page } from "puppeteer-core";
import { click, text, type } from "./browser.ts";
import {
  DASSIE,
  dassie,
  HTTPS_PORT,
  missingTools,
  NO_BOOTSTRAP_NODES,
  NO_RELAY,
  PEBBLE_API_ROOT,
  PEBBLE_DIRECTORY,
  Process,
  run,
  startActivated,
  startBrowser,
  startPebble,
  waitForPort,
  waitForSocket,
} from "./harness.ts";

const missing = await missingTools();
if (missing) console.warn(`skipping the end-to-end tests: ${missing}`);

function expectedNodeId(phrase: string): string {
  return nodeId(publicKey(deriveNodeKey(phrase)));
}

/** The certificate Pebble issues chains to the root Pebble generated. */
async function assertServedByPebble(): Promise<void> {
  const apiRoot = await readFile(PEBBLE_API_ROOT, "utf8");
  const dispatcher = new Agent({ connect: { ca: apiRoot } });
  const response = await fetch("https://localhost:15000/roots/0", {
    dispatcher,
  });
  const issuingRoot = await response.text();
  await dispatcher.close();
  // Checked against that root only, and for the name dassie.test.
  const tls = connect({
    host: "127.0.0.1",
    port: HTTPS_PORT,
    servername: "dassie.test",
    ca: [issuingRoot],
  });
  try {
    await new Promise<void>((resolve, reject) => {
      tls.once("secureConnect", resolve);
      tls.once("error", reject);
    });
  } finally {
    tls.destroy();
  }
}

/** Generates a wallet in the setup wizard; returns its phrase. */
async function generateWallet(page: Page): Promise<string> {
  await click(page, "generate-wallet");
  const phrase = await text(page, "mnemonic");
  const words = phrase.split(/\s+/);
  assertEquals(words.length, 12);
  await click(page, "backed-up");
  const number = Number(await text(page, "verify-word-number"));
  await type(page, "verify-input", words[number - 1]);
  await click(page, "verify-submit");
  return phrase;
}

async function logIn(page: Page, phrase: string): Promise<void> {
  await type(page, "login-input", phrase);
  await click(page, "login-submit");
}

async function stopAll(processes: Process[]): Promise<void> {
  for (const process of processes.reverse()) {
    const { code, output } = await process.stop();
    if (process.name === "dassie" && code !== 0) {
      throw new Error(`the daemon exited with ${code}:\n${output}`);
    }
  }
}

test(
  "a fresh install reaches setup complete with a certificate from the ACME CA",
  { skip: missing !== undefined },
  async () => {
    const verify = await run(DASSIE, ["verify-install"]);
    assert(
      verify.code === 0,
      "the binary must carry the wallet: node --run build:e2e",
    );

    const dir = await mkdtemp(join(tmpdir(), "dassie-e2e-"));
    const processes = await startPebble();
    let daemon = await startActivated(dir);
    processes.push(daemon);
    const browser = await startBrowser(dir);
    try {
      // `dassie init` configures the node and obtains the certificate.
      const output = await dassie(dir, [
        "init",
        "--hostname",
        "dassie.test",
        "--email",
        "e2e@dassie.test",
        "--agree-tos",
        "--acme-directory",
        PEBBLE_DIRECTORY,
      ]);
      const setupUrl = output.split("\n").find((line) =>
        line.startsWith("https://")
      );
      assertMatch(
        setupUrl ?? "",
        new RegExp(`^https://dassie\\.test:${HTTPS_PORT}/setup/[0-9a-f]{64}$`),
      );
      await assertServedByPebble();

      // The browser sets the node up.
      const page = await browser.newPage();
      await page.goto(setupUrl!, { waitUntil: "load" });
      const phrase = await generateWallet(page);
      // Without a ledger yet, the owner first chooses one.
      await text(page, "page-create-ledger");
      assertEquals(await text(page, "node-id"), expectedNodeId(phrase));

      // Logging out and in again, with a wrong phrase first.
      await click(page, "logout");
      await logIn(page, generateMnemonic());
      assertMatch(await text(page, "login-error"), /does not belong/);
      await logIn(page, phrase);
      assertEquals(await text(page, "node-id"), expectedNodeId(phrase));

      // A restart keeps the identity, the session and the certificate.
      daemon = await restart(processes, daemon, dir);
      await page.reload();
      assertEquals(await text(page, "node-id"), expectedNodeId(phrase));
      await assertServedByPebble();
      const setup = await page.evaluate(async () => {
        const response = await fetch("/api/setup", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: "{}",
        });
        return response.status;
      });
      assertEquals(setup, 404, "setup is gone once done");
    } finally {
      await browser.close();
      await stopAll(processes);
      await rm(dir, { recursive: true, force: true });
    }
  },
);

async function restart(
  processes: Process[],
  daemon: Process,
  dir: string,
): Promise<Process> {
  const { code, output } = await daemon.stop();
  assertEquals(code, 0, output);
  processes.splice(processes.indexOf(daemon), 1);
  const started = await startActivated(dir);
  processes.push(started);
  return started;
}

test(
  "the standalone runtime with its self-signed certificate, recovering a wallet",
  { skip: missing !== undefined },
  async () => {
    const dir = await mkdtemp(join(tmpdir(), "dassie-e2e-"));
    const daemon = new Process("dassie", DASSIE, [
      "daemon",
      "--http-port",
      "18880",
      "--https-port",
      "18843",
    ], {
      env: {
        DASSIE_STATE_DIRECTORY: `${dir}/state`,
        DASSIE_IPC_SOCKET_PATH: `${dir}/ipc.sock`,
        DASSIE_BOOTSTRAP_NODES: NO_BOOTSTRAP_NODES,
        DASSIE_RELAY: NO_RELAY,
      },
    });
    // The daemon listens on its ports before it publishes its IPC socket,
    // which it does only once it answers there, so `init` waits for both.
    await waitForPort(18843);
    await waitForSocket(`${dir}/ipc.sock`);
    const browser = await startBrowser(dir);
    try {
      const output = await dassie(dir, [
        "init",
        "--hostname",
        "localhost",
        "--skip-acme",
      ]);
      const setupUrl = output.split("\n").find((line) =>
        line.startsWith("https://")
      );
      assertMatch(
        setupUrl ?? "",
        /^https:\/\/localhost:18843\/setup\/[0-9a-f]{64}$/,
      );
      const page = await browser.newPage();
      await page.goto(setupUrl!, { waitUntil: "load" });
      const phrase =
        "pulp junk march life divorce celery flower bar one angry claw crush";
      await click(page, "recover-wallet");
      await type(page, "recover-input", "not a passphrase");
      await click(page, "recover-submit");
      await text(page, "setup-error");
      await type(page, "recover-input", phrase);
      await click(page, "recover-submit");
      assertEquals(
        await text(page, "node-id"),
        "272r4ccpkqycyl2uoslxjd3i2e5hhyvz4j2wxui",
      );
    } finally {
      await browser.close();
      await stopAll([daemon]);
      await rm(dir, { recursive: true, force: true });
    }
  },
);
