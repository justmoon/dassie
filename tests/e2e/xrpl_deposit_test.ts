/**
 * End to end (spec/areas/testing.md#end-to-end): the wallet's Receive page
 * on an XRP Ledger testnet node (spec/areas/wallet.md#pages). The
 * `xrpl-settlement` scenario runs on the node's fake ledger; a headless
 * browser signed in to `d1` finds the node's deposit address, and asks the
 * faucet for test XRP, which the node books as a deposit once the ledger
 * shows it (spec/areas/xrpl.md#deposits-and-withdrawals). Set
 * `DASSIE_XRPL_TESTNET=1` to run it against the testnet instead, by hand,
 * and `DASSIE_E2E_SCREENSHOTS` to a directory for pictures of the page.
 */

import { test } from "node:test";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { assert, assertEquals, assertMatch } from "@std/assert";
import { LedgerConnection } from "@dassie/rpc/proto";
import { Scenario, type ScenarioNode } from "@dassie/dev/scenario";
import type { Page } from "puppeteer-core";
import { click, count, signIn, text, until } from "./browser.ts";
import {
  DASSIE,
  FAKE_LEDGER,
  findChrome,
  missingScenarioTools,
  SCENARIO_PORT_OFFSET,
  startBrowser,
} from "./harness.ts";

const LEDGER = "xrpl-testnet+xrp";
const PORT_OFFSET = SCENARIO_PORT_OFFSET + 800;
/** The faucet is asked at most once a minute, the account's making included. */
const FAUCET_PACE_MS = 60_000;

async function missingTools(): Promise<string | undefined> {
  const missing = await missingScenarioTools();
  if (missing) return missing;
  if (await findChrome()) return undefined;
  if (process.env.CI) {
    throw new Error("end-to-end tests cannot run: missing Chrome (CHROME_BIN)");
  }
  return "missing Chrome (CHROME_BIN)";
}

const missing = await missingTools();
const testnet = process.env.DASSIE_XRPL_TESTNET === "1";
const screenshots = process.env.DASSIE_E2E_SCREENSHOTS;

/** Keeps a picture of the whole page, if pictures are asked for. */
async function picture(page: Page, name: string): Promise<void> {
  if (!screenshots) return;
  await mkdir(screenshots, { recursive: true });
  await writeFile(
    `${screenshots}/${name}.png`,
    await page.screenshot({ fullPage: true }),
  );
}

/** A node's XRPL ledger, as its owner sees it. */
async function ledger(node: ScenarioNode) {
  const { ledgers } = await node.rpc.call("listLedgers", {});
  const found = ledgers.find((l) => l.id === LEDGER);
  assert(found, `${node.name} has no ${LEDGER}`);
  return found;
}

test(
  "the Receive page shows the XRP deposit address and asks the faucet",
  { skip: missing !== undefined },
  async () => {
    const scenario = await Scenario.temporary("xrpl-settlement", {
      dassie: DASSIE,
      fakeLedger: testnet ? undefined : FAKE_LEDGER,
      portOffset: PORT_OFFSET,
    });
    const dir = await mkdtemp(join(tmpdir(), "dassie-xrpl-deposit-"));
    const browser = await startBrowser(dir);
    try {
      const [d1] = scenario.nodes;
      await scenario.until("funded", async () => {
        const view = await ledger(d1);
        return view.connection === LedgerConnection.SYNCED &&
          view.ownerBalance > 0n;
      }, 120_000);
      const fundedAt = Date.now();
      const funded = await ledger(d1);
      assert(funded.faucet, "the testnet has a faucet");

      const page = await browser.newPage();
      const errors: string[] = [];
      page.on("pageerror", (error) => errors.push(String(error)));
      await signIn(page, d1);
      await page.goto(`${d1.url}/receive?tab=xrpl-testnet`, {
        waitUntil: "load",
      });
      assertEquals(
        await text(page, `deposit-address-${LEDGER}`),
        funded.depositAddress,
      );
      assertMatch(funded.depositAddress, /^r/);
      assertEquals(await count(page, `deposit-qr-${LEDGER}`), 1);
      assertMatch(await text(page, `deposit-tag-${LEDGER}`), /^Not needed/);
      assertMatch(
        await text(page, `deposit-warning-${LEDGER}`),
        /Send only XRP/,
      );
      assertEquals(await count(page, `deposit-unfunded-${LEDGER}`), 0);
      await picture(page, "receive-xrp");
      await page.setViewport({ width: 390, height: 844 });
      await picture(page, "receive-xrp-phone");
      await page.setViewport({ width: 800, height: 600 });

      // The faucet made the account: it is asked again only a minute on.
      const wait = fundedAt + FAUCET_PACE_MS - Date.now();
      if (wait > 0) await new Promise((resolve) => setTimeout(resolve, wait));
      await click(page, "faucet");
      await until(
        page,
        "the faucet's XRP booked as a deposit",
        async () => (await ledger(d1)).ownerBalance,
        (balance) => balance > funded.ownerBalance,
        60_000,
      );
      await picture(page, "receive-xrp-faucet");
      assertEquals(errors, []);
    } finally {
      await browser.close();
      await rm(dir, { recursive: true, force: true });
      const failures = await scenario.stop();
      assertEquals(failures, [], failures.join("\n"));
    }
  },
);
