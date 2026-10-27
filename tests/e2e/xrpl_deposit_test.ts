/**
 * End to end (spec/areas/testing.md#end-to-end): the wallet's Receive page
 * on an XRP Ledger testnet node (spec/areas/wallet.md#pages). The
 * `xrpl-settlement` scenario runs on the node's fake ledger; a headless
 * browser signed in to `d1` finds the node's deposit address, and asks the
 * faucet for test XRP. The faucet made the account a moment ago, and it is
 * asked at most once a minute, so the node says so and asks nothing
 * (spec/areas/xrpl.md#deposits-and-withdrawals): the button reaches the
 * node, and the page tells why. Asking again after the minute, and booking
 * what the faucet sends, is the node's tests' (`tests/xrpl.rs`), on a
 * hand-moved clock. Set
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

const LEDGER = "xrpl-testnet-xrp+xrp";
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
    // The node asks the faucet for the account once it starts, so a minute
    // from now it would ask again.
    const started = Date.now();
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
      const funded = await ledger(d1);
      assert(funded.faucet, "the testnet has a faucet");

      const page = await browser.newPage();
      const errors: string[] = [];
      page.on("pageerror", (error) => errors.push(String(error)));
      await signIn(page, d1);
      await page.goto(`${d1.url}/receive?tab=xrpl-testnet-xrp`, {
        waitUntil: "load",
      });
      assertEquals(
        await text(page, `deposit-address-${LEDGER}`),
        funded.depositAddress,
      );
      assertMatch(funded.depositAddress, /^r/);
      assertEquals(await count(page, `deposit-qr-${LEDGER}`), 1);
      assertMatch(await text(page, `deposit-tag-${LEDGER}`), /^Not needed/);
      // Test money, as the node reports the ledger's realm.
      const warning = await text(page, `deposit-warning-${LEDGER}`);
      assertMatch(warning, /Send only XRP/);
      assertMatch(warning, /Test XRP has no value/);
      assertEquals(await text(page, `deposit-testnet-${LEDGER}`), "Testnet");
      assertEquals(await count(page, `deposit-unfunded-${LEDGER}`), 0);
      await picture(page, "receive-xrp");
      await page.setViewport({ width: 390, height: 844 });
      await picture(page, "receive-xrp-phone");
      await page.setViewport({ width: 800, height: 600 });

      // The faucet made the account a moment ago: it is asked again only a
      // minute on, and the page says why.
      assert(
        Date.now() - started < FAUCET_PACE_MS,
        "a minute passed before the page was ready: the node would ask the faucet again",
      );
      await click(page, "faucet");
      await until(
        page,
        "the node's refusal on the page",
        () => page.evaluate(() => document.body.innerText),
        (body) => body.includes("the faucet was asked less than a minute ago"),
      );
      await picture(page, "receive-xrp-faucet");
      // Nothing came from the faucet. The owner's funds may still have paid
      // a fee meanwhile: the scenario's two nodes peer over XRP, and the
      // junior's advance settlement goes out whenever the peering is made,
      // before or after `funded` was read.
      const after = (await ledger(d1)).ownerBalance;
      assert(
        after <= funded.ownerBalance,
        `the owner's funds grew from ${funded.ownerBalance} to ${after}`,
      );
      assertEquals(errors, []);
    } finally {
      await browser.close();
      await rm(dir, { recursive: true, force: true });
      const failures = await scenario.stop();
      assertEquals(failures, [], failures.join("\n"));
    }
  },
);
