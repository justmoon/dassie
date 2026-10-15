/**
 * The M6b done criterion (spec 7 §8): the reference Open Payments client,
 * `@interledger/open-payments`, runs the full third-party flow from one
 * node to another, with consent given in the wallet.
 *
 * 1. On d2, a non-interactive grant and an incoming payment.
 * 2. On d1, an interactive outgoing-payment grant: Chrome, signed in to
 *    d1's wallet, opens the interaction and approves; the client's finish
 *    URI, served here, gets the interaction hash and reference.
 * 3. The client continues the grant, quotes on d1 for d2's incoming
 *    payment, and pays it with an outgoing payment.
 * 4. The payment completes, the receiver got it, and the grant's spend
 *    reads back.
 * 5. The owner revokes the grant on the Connected Apps page, and its token
 *    stops working.
 * 6. A second request is denied: the app learns it, and cannot continue.
 */

import { assert, assertEquals } from "@std/assert";
import { encodeBase64Url, encodeHex } from "@std/encoding";
import { generateKeyPairSync } from "node:crypto";
import { loginSecret } from "@dassie/identity-wasm";
import {
  createAuthenticatedClient,
  isFinalizedGrantWithAccessToken,
  isPendingGrant,
} from "@interledger/open-payments";
import { certificate, Scenario, type ScenarioNode } from "@dassie/dev/scenario";
import type { Page } from "@astral/astral";
import { click, text, until } from "./browser.ts";
import {
  DASSIE,
  missingTools,
  SCENARIO_PORT_OFFSET,
  startBrowser,
} from "./harness.ts";

/** Ports of their own, beside the other suites'. */
const PORT_OFFSET = SCENARIO_PORT_OFFSET + 400;
const CLIENT_PORT = 18_961;
/** Several packets, within the credit the nodes extend each other. */
const AMOUNT = 250_000_000n;
/** What a quote for it debits: the probed one for one, less STREAM's 1%
 * slippage, rounded up. What the payment does not send goes back to the
 * grant once it completes. */
const QUOTED = 252_525_253n;

const missing = await missingTools();

function trustFetch(ca: string): () => void {
  const original = globalThis.fetch;
  const client = Deno.createHttpClient({ caCerts: [ca] });
  globalThis.fetch = (input, init) => original(input, { ...init, client });
  return () => {
    globalThis.fetch = original;
    client.close();
  };
}

/**
 * The client's wallet address, its keys, and its finish URI, which keeps
 * the query the owner's return carries.
 */
async function serveClient(scenario: Scenario, jwk: object) {
  const layout = scenario.options.layout;
  const tls = await certificate(
    layout.tls,
    "third-party",
    ["localhost"],
    layout.caRoot,
  );
  const origin = `https://localhost:${CLIENT_PORT}`;
  const finished: URLSearchParams[] = [];
  const server = Deno.serve({
    port: CLIENT_PORT,
    hostname: "127.0.0.1",
    cert: tls.certificatePem,
    key: tls.keyPem,
    onListen: () => {},
  }, (request) => {
    const url = new URL(request.url);
    if (url.pathname === "/client/jwks.json") {
      return Response.json({ keys: [jwk] });
    }
    if (url.pathname === "/client") {
      return Response.json({
        id: `${origin}/client`,
        publicName: "Budget App",
        assetCode: "USD",
        assetScale: 9,
        authServer: `${origin}/auth`,
        resourceServer: origin,
      });
    }
    if (url.pathname === "/finish") {
      finished.push(url.searchParams);
      return new Response("Back at the app.", {
        headers: { "content-type": "text/plain" },
      });
    }
    return new Response("Not Found", { status: 404 });
  });
  return {
    server,
    walletAddress: `${origin}/client`,
    finish: `${origin}/finish`,
    finished,
  };
}

/** The interaction hash (RFC 9635 §4.2.3), as unpadded base64url. */
async function interactionHash(parts: string[]): Promise<string> {
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(parts.join("\n")),
  );
  return encodeBase64Url(new Uint8Array(digest));
}

/** The HTTP status a client call failed with; it must fail. */
async function status(call: Promise<unknown>): Promise<number | undefined> {
  try {
    await call;
  } catch (error) {
    return (error as { status?: number }).status;
  }
  throw new Error("the call succeeded");
}

/** Signs the browser in to `node`'s wallet at `origin`. */
async function signIn(page: Page, node: ScenarioNode, origin: string) {
  await page.goto(origin, { waitUntil: "load" });
  const proof = encodeHex(loginSecret(node.key));
  const status = await page.evaluate(
    async (proof: string) => {
      const response = await fetch("/api/login", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ loginAuthorizationSignature: proof }),
      });
      return response.status;
    },
    { args: [proof] },
  );
  assertEquals(status, 200);
}

Deno.test({
  name: "a third-party client pays from one node to another with consent",
  ignore: missing !== undefined,
  sanitizeOps: false,
  sanitizeResources: false,
}, async () => {
  const scenario = await Scenario.temporary("two-nodes", {
    dassie: DASSIE,
    portOffset: PORT_OFFSET,
  });
  const restore = trustFetch(await Deno.readTextFile(scenario.caFile));
  const { publicKey, privateKey } = generateKeyPairSync("ed25519");
  const jwk = {
    ...publicKey.export({ format: "jwk" }),
    kid: "app-key",
    alg: "EdDSA",
    use: "sig",
  };
  const app = await serveClient(scenario, jwk);
  const dir = await Deno.makeTempDir({ prefix: "dassie-third-party-" });
  const browser = await startBrowser(dir);
  try {
    const [d1, d2] = scenario.nodes;
    // `localhost`, which Deno resolves on every system; the ports keep
    // the two apart.
    for (const node of [d1, d2]) {
      await node.rpc.call("setHostname", { hostname: "localhost" });
    }
    const payer = `https://localhost:${d1.httpsPort}`;
    const payee = `https://localhost:${d2.httpsPort}`;
    const op = await createAuthenticatedClient({
      walletAddressUrl: app.walletAddress,
      privateKey,
      keyId: "app-key",
    });

    // 1. An incoming payment on d2.
    const payeeAddress = await op.walletAddress.get({
      url: `${payee}/.well-known/pay`,
    });
    const incomingGrant = await op.grant.request(
      { url: payeeAddress.authServer },
      {
        access_token: {
          access: [{ type: "incoming-payment", actions: ["create", "read"] }],
        },
      },
    );
    assert(isFinalizedGrantWithAccessToken(incomingGrant));
    const incoming = await op.incomingPayment.create(
      {
        url: payeeAddress.resourceServer,
        accessToken: incomingGrant.access_token.value,
      },
      {
        walletAddress: payeeAddress.id,
        incomingAmount: {
          value: `${AMOUNT}`,
          assetCode: "USD",
          assetScale: 9,
        },
      },
    );

    // 2. An outgoing-payment grant on d1, which waits for the owner.
    const payerAddress = await op.walletAddress.get({
      url: `${payer}/.well-known/pay`,
    });
    const start = new Date(Date.now() - 60_000).toISOString();
    const pending = await op.grant.request({ url: payerAddress.authServer }, {
      access_token: {
        access: [
          {
            type: "outgoing-payment",
            actions: ["create", "read", "list"],
            identifier: payerAddress.id,
            limits: {
              debitAmount: {
                value: "1000000000",
                assetCode: "USD",
                assetScale: 9,
              },
              interval: `R/${start}/P1D`,
            },
          },
          { type: "quote", actions: ["create", "read"] },
        ],
      },
      interact: {
        start: ["redirect"],
        finish: { method: "redirect", uri: app.finish, nonce: "app-nonce" },
      },
    });
    assert(isPendingGrant(pending), "the grant waits for the owner");

    // The owner approves in the wallet.
    const page = await browser.newPage();
    await signIn(page, d1, payer);
    await page.goto(pending.interact.redirect, { waitUntil: "load" });
    await until(
      page,
      "the consent page names the app",
      () => text(page, "consent-client"),
      (name) => name === "Budget App",
    );
    await click(page, "consent-approve");
    for (let tries = 0; app.finished.length === 0 && tries < 100; tries++) {
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    const back = app.finished[0];
    assert(back, "the owner came back to the app");
    const reference = back.get("interact_ref") ?? "";
    assertEquals(
      back.get("hash"),
      await interactionHash([
        "app-nonce",
        pending.interact.finish,
        reference,
        payerAddress.authServer,
      ]),
    );

    // 3. The client continues, quotes, and pays.
    await new Promise((resolve) =>
      setTimeout(resolve, (pending.continue.wait ?? 0) * 1_000)
    );
    const grant = await op.grant.continue({
      url: pending.continue.uri,
      accessToken: pending.continue.access_token.value,
    }, { interact_ref: reference });
    assert(isFinalizedGrantWithAccessToken(grant), "the grant is approved");
    const accessToken = grant.access_token.value;
    const quote = await op.quote.create(
      { url: payerAddress.resourceServer, accessToken },
      { walletAddress: payerAddress.id, receiver: incoming.id, method: "ilp" },
    );
    assertEquals(quote.debitAmount.value, `${QUOTED}`);
    assertEquals(quote.receiveAmount.value, `${AMOUNT}`);
    const created = await op.outgoingPayment.create(
      { url: payerAddress.resourceServer, accessToken },
      { walletAddress: payerAddress.id, quoteId: quote.id },
    );
    assertEquals(created.grantSpentDebitAmount?.value, `${QUOTED}`);

    // 4. It completes, and the receiver has it.
    let payment = created;
    for (let tries = 0; tries < 300; tries++) {
      payment = await op.outgoingPayment.get({ url: created.id, accessToken });
      if (payment.sentAmount.value === `${AMOUNT}` || payment.failed) break;
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    assertEquals(payment.failed, false);
    assertEquals(payment.sentAmount.value, `${AMOUNT}`);
    const received = await op.incomingPayment.get({
      url: incoming.id,
      accessToken: incomingGrant.access_token.value,
    });
    assertEquals(received.receivedAmount.value, `${AMOUNT}`);
    assertEquals(received.completed, true);
    const spent = await op.outgoingPayment.getGrantSpentAmounts({
      url: payerAddress.resourceServer,
      accessToken,
    });
    assertEquals(spent.spentDebitAmount?.value, `${AMOUNT}`);

    // The owner sees the app among the connected ones.
    await page.goto(`${payer}/grants`, { waitUntil: "load" });
    await until(
      page,
      "the connected app shows what it spent",
      () => text(page, "grant-spent"),
      (spent) => spent.includes("$ 0.25 of $ 1.00 spent"),
    );

    // 5. The owner revokes it: the app's token no longer works.
    await click(page, "grant-revoke");
    await click(page, "grant-revoke-confirm");
    await until(
      page,
      "no app is connected",
      () => text(page, "grants-empty"),
      (empty) => empty !== "",
    );
    assertEquals(
      await status(op.outgoingPayment.get({ url: created.id, accessToken })),
      401,
    );

    // 6. Asked again, the owner denies: the app learns it on its return,
    // and continuing is refused.
    const again = await op.grant.request({ url: payerAddress.authServer }, {
      access_token: {
        access: [{
          type: "outgoing-payment",
          actions: ["create"],
          identifier: payerAddress.id,
        }],
      },
      interact: {
        start: ["redirect"],
        finish: { method: "redirect", uri: app.finish, nonce: "deny-nonce" },
      },
    });
    assert(isPendingGrant(again));
    await page.goto(again.interact.redirect, { waitUntil: "load" });
    await until(
      page,
      "the consent page asks again",
      () => text(page, "consent-client"),
      (name) => name === "Budget App",
    );
    await click(page, "consent-deny");
    for (let tries = 0; app.finished.length < 2 && tries < 100; tries++) {
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    assertEquals(app.finished[1]?.get("result"), "grant_rejected");
    await new Promise((resolve) =>
      setTimeout(resolve, (again.continue.wait ?? 0) * 1_000)
    );
    assertEquals(
      await status(op.grant.continue({
        url: again.continue.uri,
        accessToken: again.continue.access_token.value,
      })),
      401,
    );
  } finally {
    await browser.close();
    await app.server.shutdown();
    restore();
    await scenario.stop();
    await Deno.remove(dir, { recursive: true }).catch(() => {});
  }
});
