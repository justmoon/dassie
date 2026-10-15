// dassie.land in a headless browser: the built site (`deno task
// build:website`), served with the headers and trailing slashes of its
// vercel.json, so its CSP applies. The STREAM toy runs a payment on the
// WASM STREAM implementation to the end.

import { assert, assertEquals, assertMatch } from "@std/assert";
import { launch, type Page } from "@astral/astral";
import { contentType } from "@std/media-types";
import { extname, join } from "@std/path";
import { click, count, text, type, until } from "./browser.ts";
import { findChrome } from "./harness.ts";

const site = new URL("../../apps/website/", import.meta.url).pathname;
const dist = join(site, "dist");

async function missing(): Promise<string | undefined> {
  const reasons = [];
  if (!(await findChrome())) reasons.push("Chrome (CHROME_BIN)");
  try {
    await Deno.stat(join(dist, "index.html"));
  } catch {
    reasons.push(`${dist} (deno task build:website)`);
  }
  if (reasons.length === 0) return undefined;
  const reason = `missing ${reasons.join(", ")}`;
  if (Deno.env.get("CI")) {
    throw new Error(`website tests cannot run: ${reason}`);
  }
  console.warn(`skipping the website tests: ${reason}`);
  return reason;
}

interface Rule {
  source: string;
  headers: { key: string; value: string }[];
}

/** Serves dist/ as Vercel would, with vercel.json's headers. */
async function serve(): Promise<Deno.HttpServer<Deno.NetAddr>> {
  const config = JSON.parse(
    await Deno.readTextFile(join(site, "vercel.json")),
  ) as { headers: Rule[]; trailingSlash: boolean };
  assert(config.trailingSlash);
  const rules = config.headers.map((rule) => ({
    pattern: new RegExp(`^${rule.source}$`),
    headers: rule.headers,
  }));
  return Deno.serve(
    { port: 0, hostname: "127.0.0.1", onListen() {} },
    async (request) => {
      const path = decodeURIComponent(new URL(request.url).pathname);
      if (path.includes("..")) return new Response("bad path", { status: 400 });
      if (!path.endsWith("/") && extname(path) === "") {
        return Response.redirect(new URL(`${path}/`, request.url), 308);
      }
      const file = join(dist, path.endsWith("/") ? `${path}index.html` : path);
      let body: Uint8Array<ArrayBuffer>;
      try {
        body = await Deno.readFile(file);
      } catch {
        return new Response("not found", { status: 404 });
      }
      const headers = new Headers({
        "content-type": contentType(extname(file)) ??
          "application/octet-stream",
      });
      for (const rule of rules) {
        if (!rule.pattern.test(path)) continue;
        for (const { key, value } of rule.headers) headers.set(key, value);
      }
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
  page.addEventListener("pageerror", (event) => {
    problems.push(String(event.detail));
  });
  page.addEventListener("console", (event) => {
    if (event.detail.type === "error") problems.push(event.detail.text);
  });
  await page.unsafelyGetCelestialBindings().Page
    .addScriptToEvaluateOnNewDocument({
      source: `document.addEventListener("securitypolicyviolation", (event) =>
        console.error("CSP: " + event.violatedDirective + " " + event.blockedURI));`,
    });
  return problems;
}

/** Removes `dir`, which Chrome may still write to for a moment after it exits. */
async function removeEventually(dir: string): Promise<void> {
  for (let attempt = 1;; attempt++) {
    try {
      await Deno.remove(dir, { recursive: true });
      return;
    } catch (error) {
      if (attempt === 10) throw error;
      await new Promise((resolve) => setTimeout(resolve, 200));
    }
  }
}

const reason = await missing();

Deno.test({
  name: "dassie.land: the pages, and a STREAM payment in the toy",
  ignore: reason !== undefined,
  sanitizeResources: false,
  sanitizeOps: false,
  async fn(t) {
    const server = await serve();
    const base = `http://127.0.0.1:${server.addr.port}`;
    const dir = await Deno.makeTempDir({ prefix: "dassie-website-" });
    const browser = await launch({
      path: await findChrome(),
      args: ["--no-sandbox", `--user-data-dir=${dir}/chrome`],
    });
    try {
      const page = await browser.newPage();
      const problems = await watch(page);

      await t.step("the pages link to each other", async () => {
        await page.goto(`${base}/`, { waitUntil: "load" });
        assertEquals(
          await page.evaluate(() => document.querySelector("h1")?.textContent),
          "What is Dassie?",
        );
        // Without the slash, as the original's links had it.
        await page.goto(`${base}/guides/quick-start`, { waitUntil: "load" });
        assertEquals(new URL(page.url).pathname, "/guides/quick-start/");
        const body = await page.evaluate(() => document.body.innerText);
        assertMatch(
          body,
          /curl --proto =https --tlsv1\.2 -sSf https:\/\/sh\.dassie\.land \| sh/,
        );
        assertMatch(body, /sudo dassie init/);
        // The stylesheet applies: it comes from the site's own origin.
        const background = await page.evaluate(() =>
          getComputedStyle(document.querySelector("pre")!).borderStyle
        );
        assertEquals(background, "solid");
        assertEquals(problems, []);
      });

      await t.step("the toy delivers the default payment", async () => {
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

      await t.step(
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

      await t.step("settings out of range are refused", async () => {
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
  },
});
