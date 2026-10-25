// dassie.land in a headless browser: the built site (`node --run
// build:website`), served as its bucket serves it: with the headers of
// headers.json, so its CSP applies, and trailing slashes. The architecture map draws and answers
// clicks, and the STREAM toy runs a payment on the WASM STREAM
// implementation to the end.

import { test } from "node:test";
import { mkdtemp, readFile, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { extname, join } from "node:path";
import process from "node:process";

import { assert, assertEquals, assertMatch } from "@std/assert";
import { serve, type Server } from "@dassie/dev/serve";
import puppeteer, { type Page } from "puppeteer-core";
import { click, count, text, type, until } from "./browser.ts";
import { findChrome } from "./harness.ts";

const site = new URL("../../apps/website/", import.meta.url).pathname;
const dist = join(site, "dist");

async function missing(): Promise<string | undefined> {
  const reasons = [];
  if (!(await findChrome())) reasons.push("Chrome (CHROME_BIN)");
  try {
    await stat(join(dist, "index.html"));
  } catch {
    reasons.push(`${dist} (node --run build:website)`);
  }
  if (reasons.length === 0) return undefined;
  const reason = `missing ${reasons.join(", ")}`;
  if (process.env.CI) {
    throw new Error(`website tests cannot run: ${reason}`);
  }
  console.warn(`skipping the website tests: ${reason}`);
  return reason;
}

/** The types of the files the site has. */
const TYPES: Record<string, string> = {
  ".css": "text/css; charset=UTF-8",
  ".html": "text/html; charset=UTF-8",
  ".ico": "image/vnd.microsoft.icon",
  ".js": "text/javascript; charset=UTF-8",
  ".json": "application/json; charset=UTF-8",
  ".png": "image/png",
  ".svg": "image/svg+xml",
  ".wasm": "application/wasm",
  ".webp": "image/webp",
  ".woff2": "font/woff2",
};

/**
 * Serves dist/ as the bucket behind the load balancer does: every response
 * with headers.json's headers, and a page's path without its slash
 * redirected to the path with it.
 */
async function serveSite(): Promise<Server> {
  const siteHeaders = JSON.parse(
    await readFile(join(site, "headers.json"), "utf8"),
  ) as { key: string; value: string }[];
  return await serve(
    { port: 0, hostname: "127.0.0.1" },
    async (request) => {
      const path = decodeURIComponent(new URL(request.url).pathname);
      if (path.includes("..")) return new Response("bad path", { status: 400 });
      if (!path.endsWith("/") && extname(path) === "") {
        return Response.redirect(new URL(`${path}/`, request.url), 308);
      }
      const file = join(dist, path.endsWith("/") ? `${path}index.html` : path);
      let body: Uint8Array<ArrayBuffer>;
      try {
        body = await readFile(file);
      } catch {
        return new Response("not found", { status: 404 });
      }
      const headers = new Headers({
        "content-type": TYPES[extname(file)] ?? "application/octet-stream",
      });
      for (const { key, value } of siteHeaders) headers.set(key, value);
      return new Response(body, { headers });
    },
  );
}

/**
 * Collects what the page logs as errors, throws, or refuses by its CSP,
 * from the start of every document it loads from now on.
 */
async function watch(page: Page): Promise<string[]> {
  const problems: string[] = [];
  page.on("pageerror", (error) => {
    problems.push(String(error));
  });
  page.on("console", (message) => {
    if (message.type() === "error") problems.push(message.text());
  });
  await page.evaluateOnNewDocument(
    `document.addEventListener("securitypolicyviolation", (event) =>
        console.error("CSP: " + event.violatedDirective + " " + event.blockedURI));`,
  );
  return problems;
}

/** Removes `dir`, which Chrome may still write to for a moment after it exits. */
async function removeEventually(dir: string): Promise<void> {
  for (let attempt = 1;; attempt++) {
    try {
      await rm(dir, { recursive: true, force: true });
      return;
    } catch (error) {
      if (attempt === 10) throw error;
      await new Promise((resolve) => setTimeout(resolve, 200));
    }
  }
}

const reason = await missing();

test("dassie.land: the pages, and a STREAM payment in the toy", {
  skip: reason !== undefined,
}, async (t) => {
  const server = await serveSite();
  const base = `http://127.0.0.1:${server.addr.port}`;
  const dir = await mkdtemp(join(tmpdir(), "dassie-website-"));
  const browser = await puppeteer.launch({
    executablePath: await findChrome(),
    headless: true,
    args: ["--no-sandbox", `--user-data-dir=${dir}/chrome`],
  });
  try {
    const page = await browser.newPage();
    const problems = await watch(page);

    await t.test("the pages link to each other", async () => {
      await page.goto(`${base}/`, { waitUntil: "load" });
      assertEquals(
        await page.evaluate(() => document.querySelector("h1")?.textContent),
        "Payments that grow sideways.",
      );
      // The install command's Copy button shows once its script runs.
      assertEquals(
        await page.evaluate(() =>
          document.querySelector<HTMLButtonElement>(
            '[data-testid="copy-install"]',
          )?.hidden
        ),
        false,
      );
      // Without the slash, as the original's links had it.
      await page.goto(`${base}/guides/quick-start`, { waitUntil: "load" });
      assertEquals(new URL(page.url()).pathname, "/guides/quick-start/");
      const body = await page.evaluate(() => document.body.innerText);
      assertMatch(
        body,
        /curl --proto =https --tlsv1\.2 -sSf https:\/\/sh\.dassie\.land \| sh/,
      );
      assertMatch(body, /sudo dassie init/);
      // The stylesheet applies, and its fonts load: they come from the
      // site's own origin. Commands are set on Kopje's ink.
      const background = await page.evaluate(() =>
        getComputedStyle(document.querySelector("pre")!).backgroundColor
      );
      assertEquals(background, "rgb(43, 36, 32)");
      const loaded = await page.evaluate(async () => {
        await document.fonts.ready;
        return [...document.fonts]
          .filter((font) => font.status === "loaded")
          .map((font) => font.family.replaceAll('"', ""));
      });
      assert(loaded.includes("Fraunces"), loaded.join());
      assertEquals(problems, []);
    });

    await t.test("the architecture map shows a journey and a box", async () => {
      const map = JSON.parse(
        await readFile(join(site, "src/architecture/map.json"), "utf8"),
      ) as {
        components: object;
        flows: { title: string }[];
        releases: object[];
      };
      await page.goto(`${base}/architecture/`, { waitUntil: "load" });
      assertEquals(await text(page, "panel-title"), "How to read the map");
      assertEquals(
        await count(page, "box"),
        Object.keys(map.components).length,
      );
      assertEquals(await count(page, "release"), map.releases.length);

      await click(page, "journey");
      assertEquals(await text(page, "panel-title"), map.flows[0].title);
      // A box is an SVG group, which has no click(): a click event stands in.
      await page.evaluate(() =>
        document.querySelector('[data-testid="box"][data-id="node"]')!
          .dispatchEvent(new MouseEvent("click", { bubbles: true }))
      );
      await until(
        page,
        "the panel shows the node",
        () => text(page, "panel-title"),
        (title) => title === "dassie-node",
      );
      const selected = await page.evaluate(() =>
        document.querySelector('[data-id="node"]')!.classList.contains("sel")
      );
      assert(selected);
      assertEquals(problems, []);
    });

    await t.test("the brand page draws in its own fonts, and turns to dusk", async () => {
      await page.goto(`${base}/brand/`, { waitUntil: "load" });
      assertEquals(
        await page.evaluate(() => document.querySelector("h1")?.textContent),
        "Kopje",
      );
      // Its fonts are the site's own files, which the CSP lets load.
      const loaded = await page.evaluate(async () => {
        await document.fonts.ready;
        return [...document.fonts]
          .filter((font) => font.status === "loaded")
          .map((font) => font.family.replaceAll('"', ""));
      });
      assert(loaded.includes("Fraunces"), loaded.join());
      assert(loaded.includes("Figtree"), loaded.join());
      const theme = () =>
        page.evaluate(() => document.documentElement.dataset.theme);
      assertEquals(await theme(), "light");
      await click(page, "theme-toggle");
      assertEquals(await theme(), "dark");
      assertEquals(
        await page.evaluate(() =>
          document.querySelector(".theme-toggle")?.getAttribute("aria-pressed")
        ),
        "true",
      );
      assertEquals(problems, []);
    });

    await t.test("the toy delivers the default payment", async () => {
      await page.goto(`${base}/tools/stream-toy/`, { waitUntil: "load" });
      await click(page, "toy-simulate");
      await until(
        page,
        "the run ends",
        () => text(page, "toy-status"),
        (status) => status === "Done",
      );
      assertEquals(
        await text(page, "toy-delivered"),
        "3000 of 3000 delivered",
      );
      assert((await count(page, "packet")) >= 4);
      const outcomes = await page.evaluate(() =>
        [...document.querySelectorAll('[data-testid="packet"]')].map((node) =>
          node.getAttribute("data-outcome")
        )
      );
      assertEquals(outcomes[0], "rejected");
      assert(outcomes.includes("fulfilled"), outcomes.join());
      assertMatch(await text(page, "packet-code"), /^F08 Amount Too Large$/);

      // Opened, a packet shows the STREAM packets inside, decrypted.
      await click(page, "packet-head");
      const details = await page.evaluate(() =>
        document.querySelector('[data-testid="packets"]')!.textContent
      );
      assertMatch(details!, /F08 data/);
      assertMatch(details!, /Maximum amount1000/);
      assertMatch(details!, /StreamMoney/);
      assertMatch(details!, /ConnectionNewAddress/);
      // The log viewer shows the run's lines.
      assertMatch(await text(page, "toy-log"), /money received/);
      assertEquals(problems, []);
    });

    await t.test(
      "a slow run can be paused, stepped and finished",
      async () => {
        await click(page, "toy-back");
        await type(page, "toy-amount", "20000");
        await type(page, "toy-timeDilation", "0.01");
        await click(page, "toy-simulate");
        await click(page, "toy-pause");
        assertEquals(await text(page, "toy-status"), "Paused");
        const before = await count(page, "packet");
        for (let step = 0; step < 5; step++) await click(page, "toy-step");
        await until(
          page,
          "steps send packets",
          () => count(page, "packet"),
          (now) => now > before,
        );
        await click(page, "toy-finish");
        await until(
          page,
          "the run ends",
          () => text(page, "toy-status"),
          (status) => status === "Done",
        );
        assertEquals(
          await text(page, "toy-delivered"),
          "20000 of 20000 delivered",
        );
      },
    );

    await t.test("settings out of range are refused", async () => {
      await click(page, "toy-back");
      await type(page, "toy-amount", "1.5");
      await click(page, "toy-simulate");
      assertEquals(
        await text(page, "toy-problem"),
        "The amount must be a whole number.",
      );
      assertEquals(problems, []);
    });
  } finally {
    await browser.close();
    await server.shutdown();
    await removeEventually(dir);
  }
});
