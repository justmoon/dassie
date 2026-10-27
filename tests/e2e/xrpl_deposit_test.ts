/**
 * End to end (spec/areas/testing.md#end-to-end): the wallet's Receive page
 * on a settlement hub's member, for its owner book of XRP on the XRP
 * Ledger testnet (spec/areas/wallet.md#pages). The `hub` scenario runs on
 * the node's fake ledger, and once its hub is founded a node added to it
 * joins as a member; a headless browser signed in to that member finds
 * its deposit address, and asks the faucet for test XRP. The faucet made
 * the account a moment ago, as the member started, and it is asked at
 * most once a minute, so the node says so and asks nothing
 * (spec/areas/xrpl.md#deposits-and-withdrawals): the button reaches the
 * node, and the page tells why. The added member, rather than one the
 * scenario starts with, since founding the hub may take longer than that
 * minute. Asking again after the minute, and booking what the faucet
 * sends, is the node's tests' (`tests/xrpl.rs`), on a hand-moved clock.
 *
 * The scenario runs only on the fake ledger: each run founds a hub of its
 * own, whose escrow and validators must stay off the public testnet, so
 * this test has no switch to it. Set `DASSIE_E2E_SCREENSHOTS` to a
 * directory for pictures of the page.
 */

import { test } from "node:test";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { assert, assertEquals, assertMatch } from "@std/assert";
import { HUB_OWNER_BOOK, Scenario } from "@dassie/dev/scenario";
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
import {
  fundedInOwnerBook,
  HUB_OWNER_LEDGER,
  HUB_WITHIN_MS,
  ledger,
} from "./settling.ts";

const LEDGER = HUB_OWNER_LEDGER;
const PORT_OFFSET = SCENARIO_PORT_OFFSET + 800;
/** The faucet is asked at most once a minute, the account's making included. */
const FAUCET_PACE_MS = 60_000;
/**
 * How long before that minute is over the test clicks the faucet's button
 * at the latest: the click reaches the node a moment later, and must find
 * the minute not yet passed.
 */
const FAUCET_MARGIN_MS = 15_000;

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

test(
  "the Receive page shows a hub member's XRP deposit address and asks the faucet",
  { skip: missing !== undefined },
  async () => {
    const scenario = await Scenario.temporary("hub", {
      dassie: DASSIE,
      fakeLedger: FAKE_LEDGER,
      portOffset: PORT_OFFSET,
    });
    const dir = await mkdtemp(join(tmpdir(), "dassie-xrpl-deposit-"));
    const browser = await startBrowser(dir);
    try {
      // The member asks the faucet for the account once it starts, so a
      // minute from now it would ask again.
      const started = Date.now();
      const member = await scenario.addNode();
      await scenario.until(
        "funded",
        () => fundedInOwnerBook(member),
        HUB_WITHIN_MS,
      );
      const funded = await ledger(member, LEDGER);
      assert(funded.faucet, "the testnet has a faucet");

      const page = await browser.newPage();
      const errors: string[] = [];
      page.on("pageerror", (error) => errors.push(String(error)));
      await signIn(page, member);
      await page.goto(`${member.url}/receive?tab=${HUB_OWNER_BOOK}`, {
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
        Date.now() - started < FAUCET_PACE_MS - FAUCET_MARGIN_MS,
        "nearly a minute passed before the page was ready: the node might ask the faucet again",
      );
      await click(page, "faucet");
      await until(
        page,
        "the node's refusal on the page",
        () => page.evaluate(() => document.body.innerText),
        (body) => body.includes("the faucet was asked less than a minute ago"),
      );
      await picture(page, "receive-xrp-faucet");
      // Nothing came from the faucet. The owner's funds may still have
      // fallen meanwhile: the member opens its account in the hub by a
      // deposit from this book, before or after `funded` was read.
      const after = (await ledger(member, LEDGER)).ownerBalance;
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
