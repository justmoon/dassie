// The browser build (`--target web`) loads and works in headless Chrome:
// it derives a node ID and runs a STREAM payment between two connections
// inside the page. Set CHROME_BIN to choose the browser; otherwise the
// first of google-chrome, chromium and chromium-browser on PATH is used.

import { test } from "node:test";
import { execFile } from "node:child_process";
import { readFile } from "node:fs/promises";
import process from "node:process";

import { assertEquals } from "@std/assert";
import { serve } from "@dassie/dev/serve";
import puppeteer from "puppeteer-core";

const web = new URL("../../packages/wasm/gen/web/", import.meta.url);

const page = `<!doctype html>
<meta charset="utf-8">
<script type="module">
import initIdentity, * as identity from "./dassie_identity.js";
import initStream, * as stream from "./dassie_stream.js";

try {
  await initIdentity({ module_or_path: "./dassie_identity_bg.wasm" });
  await initStream({ module_or_path: "./dassie_stream_bg.wasm" });

  const phrase = "pulp junk march life divorce celery flower bar one angry claw crush";
  const nodeId = identity.nodeId(identity.publicKey(identity.deriveNodeKey(phrase)));

  const secret = new Uint8Array(32);
  crypto.getRandomValues(secret);
  const server = new stream.StreamServer(secret, "test.server", "USD", 6, 1000000n);
  const credentials = server.generateCredentials(new Uint8Array());
  const client = new stream.StreamConnection(
    credentials.sharedSecret, credentials.destination, "test.client", "USD", 6);
  client.setExchangeRate(1n, 1n);
  const id = client.openStream();
  client.setSendMax(id, 250000n);

  // A connector with no delay: every Prepare is answered at once.
  let now = Date.now();
  for (let step = 0; step < 1000 && client.totalSent(id) < 250000n; step++) {
    const transmit = client.pollTransmit(now);
    if (!transmit) {
      now = client.pollTimeout() ?? now + 1;
      client.handleTimeout(now);
      continue;
    }
    const response = server.handlePrepare(now, transmit.packet);
    client.handleResponse(now, transmit.requestId, response);
  }
  window.__result = { nodeId, sent: String(client.totalSent(id)) };
} catch (error) {
  window.__result = { error: String(error) };
}
</script>`;

function which(name: string): Promise<string | undefined> {
  return new Promise((resolve) => {
    execFile("which", [name], (error, stdout) => {
      resolve(error ? undefined : stdout.trim());
    });
  });
}

async function findChrome(): Promise<string> {
  const configured = process.env.CHROME_BIN;
  if (configured) return configured;
  for (const name of ["google-chrome", "chromium", "chromium-browser"]) {
    const path = await which(name);
    if (path) return path;
  }
  throw new Error("no Chrome found; set CHROME_BIN");
}

test("the web build works in a browser", async () => {
  const server = await serve(
    { port: 0 },
    async (request) => {
      const path = new URL(request.url).pathname.slice(1);
      if (path === "" || path === "index.html") {
        return new Response(page, { headers: { "content-type": "text/html" } });
      }
      if (!/^[\w.-]+$/.test(path)) return new Response(null, { status: 404 });
      try {
        const body = await readFile(new URL(path, web));
        const type = path.endsWith(".wasm")
          ? "application/wasm"
          : path.endsWith(".js")
          ? "text/javascript"
          : "application/octet-stream";
        return new Response(body, { headers: { "content-type": type } });
      } catch {
        return new Response(null, { status: 404 });
      }
    },
  );
  const browser = await puppeteer.launch({
    executablePath: await findChrome(),
    headless: true,
    args: ["--no-sandbox"],
  });
  try {
    const tab = await browser.newPage();
    await tab.goto(`http://localhost:${server.addr.port}/`);
    await tab.waitForFunction(() => "__result" in globalThis);
    const result = await tab.evaluate(() =>
      (globalThis as unknown as { __result: Record<string, string> }).__result
    );
    assertEquals(result, {
      nodeId: "d1_UeCE9UMCwvVHSXdI9o0Tpz4rnidWvR",
      sent: "250000",
    });
  } finally {
    await browser.close();
    await server.shutdown();
  }
});
