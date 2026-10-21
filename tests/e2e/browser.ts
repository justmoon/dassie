/**
 * Driving the wallet in a headless browser: finding elements by their
 * `data-testid`, in whatever document the page shows, since setup and
 * login end by reloading the page, which a pending `waitForSelector` does
 * not survive.
 */

import { assertEquals } from "@std/assert";
import { loginSecret } from "@dassie/identity-wasm";
import type { ScenarioNode } from "@dassie/dev/scenario";
import type { ElementHandle, Page } from "puppeteer-core";

/** Signs the browser in to `node`'s wallet at `origin` as its owner, with
 * the node's key. */
export async function signIn(
  page: Page,
  node: ScenarioNode,
  origin = node.url,
): Promise<void> {
  await page.goto(origin, { waitUntil: "load" });
  const proof = loginSecret(node.key).toHex();
  const status = await page.evaluate(
    async (proof: string) => {
      const response = await fetch("/api/login", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ loginAuthorizationSignature: proof }),
      });
      return response.status;
    },
    proof,
  );
  assertEquals(status, 200);
}

/** How long to wait for an element, in ms. */
const WAIT_MS = 20_000;

/** The text of the element with `testid`, once it appears. */
export async function text(page: Page, testid: string): Promise<string> {
  const deadline = Date.now() + WAIT_MS;
  for (;;) {
    const found = await page.evaluate(
      (id: string) =>
        document.querySelector(`[data-testid="${id}"]`)?.textContent ?? null,
      testid,
    ).catch(() => null);
    if (found !== null) return found.trim();
    if (Date.now() > deadline) {
      const body = await page.evaluate(() => document.body.innerText)
        .catch(() => "(no document)");
      throw new Error(`no ${testid} on ${page.url()}:\n${body}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

/** Waits until `check` holds of the page's state, polling. */
export async function until<T>(
  page: Page,
  what: string,
  read: () => T | Promise<T>,
  check: (value: T) => boolean,
  timeoutMs = WAIT_MS,
): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  let last: T | undefined;
  for (;;) {
    last = await read();
    if (check(last)) return last;
    if (Date.now() > deadline) {
      const body = await page.evaluate(() => document.body.innerText)
        .catch(() => "(no document)");
      throw new Error(`timed out: ${what} (last ${String(last)}):\n${body}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

/** An attribute of the element with `testid`, once it appears. */
export async function attribute(
  page: Page,
  testid: string,
  name: string,
): Promise<string | null> {
  await text(page, testid);
  return await page.evaluate(
    (id: string, attribute: string) =>
      document.querySelector(`[data-testid="${id}"]`)?.getAttribute(
        attribute,
      ) ??
        null,
    testid,
    name,
  );
}

/** How many elements have `testid`. */
export async function count(page: Page, testid: string): Promise<number> {
  return await page.evaluate(
    (id: string) => document.querySelectorAll(`[data-testid="${id}"]`).length,
    testid,
  );
}

/** The element with `testid`, once it is in the current document. */
export async function element(
  page: Page,
  testid: string,
): Promise<ElementHandle<Element>> {
  await text(page, testid);
  const found = await page.$(`[data-testid="${testid}"]`);
  if (!found) throw new Error(`${testid} vanished`);
  return found;
}

/**
 * Clicks the element with `testid` as a script would: a notice that happens
 * to cover it, or a dialog still opening, does not get in the way.
 */
export async function click(page: Page, testid: string): Promise<void> {
  const target = await element(page, testid);
  await target.evaluate((node) => (node as HTMLElement).click());
}

/** Replaces the value of the input with `testid`. */
export async function type(
  page: Page,
  testid: string,
  value: string,
): Promise<void> {
  const input = await element(page, testid);
  await input.evaluate((node) => ((node as HTMLInputElement).value = ""));
  await input.type(value);
}
