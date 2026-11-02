/**
 * End to end (spec/areas/testing.md#end-to-end): a third-party app spends
 * from a node without a hostname, which it reaches only through a front
 * door, with the owner's consent given in the node's own wallet on
 * `localhost` (spec/designs/periphery.md#6-owner-apps,
 * spec/designs/periphery.md#7-open-payments-without-a-public-address).
 *
 * 1. The scenario's periphery node keeps the scenario's front door as its
 *    relay, and answers at `<node ID>.front-door.test` through it.
 * 2. The reference client, `@interledger/open-payments`, asks it for an
 *    outgoing-payment grant there. The app's browser follows the
 *    interaction URL to the front door's page, which says to approve the
 *    request in the Dassie app and asks for nothing.
 * 3. The owner, signed in to the node's wallet on `localhost`, sees the
 *    request waiting, opens it from the notice, and approves it.
 * 4. The front door's page sends the app's browser on to its finish URI
 *    with the interaction hash and reference; the client continues the
 *    grant, quotes for an incoming payment, and pays it.
 *
 * The incoming payment is the periphery node's own: a node reads another
 * node's as an Open Payments client known by its main name,
 * `<node ID>.dassiepay.net`, whose key the other node would fetch from the
 * public front door, which no test reaches.
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { generateKeyPairSync } from "node:crypto";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { createServer } from "node:http";
import { connect as connectTcp } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { rootCertificates } from "node:tls";
import {
  createAuthenticatedClient,
  isFinalizedGrantWithAccessToken,
  isPendingGrant,
} from "@interledger/open-payments";
import {
  Agent,
  buildConnector,
  type Dispatcher,
  getGlobalDispatcher,
  setGlobalDispatcher,
} from "undici";
import { serve } from "@dassie/dev/serve";
import { certificate, Scenario } from "@dassie/dev/scenario";
import { click, signIn, text, until } from "./browser.ts";
import {
  DASSIE,
  FRONT_DOOR_PORT,
  missingScenarioTools,
  PORT_OFFSETS,
  startBrowser,
} from "./harness.ts";

/** Ports of their own, beside the other suites'. */
const PORT_OFFSET = PORT_OFFSETS["front_door_test.ts"];
/** The front door's domain: no resolver knows it, so nothing outside this
 * test reaches it. */
const DOMAIN = "front-door.test";
/** Several packets, within the credit the nodes extend each other. */
const AMOUNT = 250_000_000n;

const missing = await missingScenarioTools();

/**
 * Sends every name under the front door's domain to the front door, on
 * its port, as a resolver that knew the domain and a front door on 443
 * would; every other name as it is. Trusts `ca` besides Node's own roots.
 * The client library fetches through the global `fetch`, which takes the
 * global dispatcher. Returns a function that puts the previous one back.
 */
function throughFrontDoor(ca: string): () => void {
  const previous: Dispatcher = getGlobalDispatcher();
  const connect = buildConnector({ ca: [...rootCertificates, ca] });
  const agent = new Agent({
    connect(options, callback) {
      const door = options.hostname.endsWith(`.${DOMAIN}`);
      connect({
        ...options,
        hostname: door ? "127.0.0.1" : options.hostname,
        port: door ? `${FRONT_DOOR_PORT}` : options.port,
        servername: options.hostname,
      }, callback);
    },
  });
  setGlobalDispatcher(agent);
  return () => {
    setGlobalDispatcher(previous);
    void agent.close();
  };
}

/**
 * A proxy for the browser, which sends it every connection as a `CONNECT`:
 * names under the front door's domain go to the front door, as a resolver
 * that knew the domain and a front door on 443 would, and every other name
 * where it says. Chrome connects to loopback without it. TLS passes
 * through: the browser talks to the front door itself.
 */
async function frontDoorProxy(): Promise<{
  url: string;
  close(): Promise<void>;
}> {
  const server = createServer((_, response) => {
    response.writeHead(405).end();
  });
  server.on("connect", (request, client, head) => {
    const target = new URL(`https://${request.url ?? ""}`);
    const door = target.hostname.endsWith(`.${DOMAIN}`);
    const upstream = connectTcp(
      door ? FRONT_DOOR_PORT : Number(target.port || "443"),
      door ? "127.0.0.1" : target.hostname,
      () => {
        client.write("HTTP/1.1 200 Connection Established\r\n\r\n");
        upstream.write(head);
        upstream.pipe(client);
        client.pipe(upstream);
      },
    );
    upstream.on("error", () => client.destroy());
    client.on("error", () => upstream.destroy());
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  return {
    url: `http://127.0.0.1:${port}`,
    close: () =>
      new Promise((resolve) => {
        server.closeAllConnections();
        server.close(() => resolve());
      }),
  };
}

/**
 * The app's wallet address, its keys, and its finish URI, which keeps the
 * query the browser brings back.
 */
async function serveApp(scenario: Scenario, jwk: object) {
  const layout = scenario.options.layout;
  const tls = await certificate(
    layout.tls,
    "front-door-app",
    ["localhost"],
    layout.caRoot,
  );
  // At a port from the system: it is known once the server listens, before
  // anything asks it.
  let origin = "";
  const finished: URLSearchParams[] = [];
  const server = await serve({
    port: 0,
    hostname: "127.0.0.1",
    cert: tls.certificatePem,
    key: tls.keyPem,
  }, (request) => {
    const url = new URL(request.url);
    if (url.pathname === "/app/jwks.json") {
      return Response.json({ keys: [jwk] });
    }
    if (url.pathname === "/app") {
      return Response.json({
        id: `${origin}/app`,
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
  origin = `https://localhost:${server.addr.port}`;
  return {
    server,
    walletAddress: `${origin}/app`,
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
  return new Uint8Array(digest).toBase64({
    alphabet: "base64url",
    omitPadding: true,
  });
}

test(
  "an app spends from a node without a hostname, with consent in its wallet",
  {
    skip: missing !== undefined,
  },
  async () => {
    const scenario = await Scenario.temporary("periphery", {
      dassie: DASSIE,
      portOffset: PORT_OFFSET,
      frontDoor: { domain: DOMAIN, port: FRONT_DOOR_PORT },
    });
    const restore = throughFrontDoor(await readFile(scenario.caFile, "utf8"));
    const { publicKey, privateKey } = generateKeyPairSync("ed25519");
    const jwk = {
      ...publicKey.export({ format: "jwk" }),
      kid: "app-key",
      alg: "EdDSA",
      use: "sig",
    };
    const app = await serveApp(scenario, jwk);
    const dir = await mkdtemp(join(tmpdir(), "dassie-front-door-"));
    // The app's browser finds the front door's names as the client does.
    const proxy = await frontDoorProxy();
    const browser = await startBrowser(dir, [`--proxy-server=${proxy.url}`]);
    try {
      const [core, periphery] = scenario.nodes;
      const name = scenario.frontDoor!.name(periphery.id);
      const payer = `https://${name}/.well-known/pay`;
      const op = await createAuthenticatedClient({
        walletAddressUrl: app.walletAddress,
        privateKey,
        keyId: "app-key",
      });

      // 1. The periphery node answers through the front door once it holds
      // its connection there, and has its uplink.
      await scenario.until("the front door reaches the node", async () => {
        const reply = await fetch(payer).catch(() => undefined);
        return reply?.status === 200;
      });
      await scenario.until(
        "the periphery node peers with its uplink",
        async () => {
          const { peers } = await periphery.rpc.call("listPeers", {});
          return peers.some((peer) => peer.node === core.id);
        },
      );
      const payerAddress = await op.walletAddress.get({ url: payer });
      assertEquals(payerAddress.id, payer);
      assertEquals(payerAddress.authServer, `https://${name}/gnap`);

      // An incoming payment, for the app to pay, through the front door.
      const incomingGrant = await op.grant.request(
        { url: payerAddress.authServer },
        {
          access_token: {
            access: [{ type: "incoming-payment", actions: ["create", "read"] }],
          },
        },
      );
      assert(isFinalizedGrantWithAccessToken(incomingGrant));
      const incoming = await op.incomingPayment.create(
        {
          url: payerAddress.resourceServer,
          accessToken: incomingGrant.access_token.value,
        },
        {
          walletAddress: payerAddress.id,
          incomingAmount: {
            value: `${AMOUNT}`,
            assetCode: "USD",
            assetScale: 9,
          },
        },
      );

      // 2. An outgoing-payment grant through the front door, which waits for
      // the owner; the app sends its browser to the front door's page.
      const start = new Date(Date.now() - 60_000).toISOString();
      const pending = await op.grant.request({ url: payerAddress.authServer }, {
        access_token: {
          access: [
            {
              type: "outgoing-payment",
              actions: ["create", "read"],
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
      assert(
        pending.interact.redirect.startsWith(`https://${name}/gnap/interact/`),
        pending.interact.redirect,
      );
      const appPage = await browser.newPage();
      await appPage.goto(pending.interact.redirect, { waitUntil: "load" });
      assertEquals(
        await appPage.title(),
        "Approve this in your Dassie app",
      );
      await until(
        appPage,
        "the front door's page waits for the owner",
        () =>
          appPage.evaluate(() =>
            document.getElementById("status")?.textContent ?? ""
          ),
        (status) => status.startsWith("Waiting for the owner"),
      );

      // 3. The owner, in the node's own wallet on `localhost`, sees the
      // request waiting, and approves it.
      const wallet = `https://localhost:${periphery.httpsPort}`;
      const ownerPage = await browser.newPage();
      await signIn(ownerPage, periphery, wallet);
      await ownerPage.goto(`${wallet}/`, { waitUntil: "load" });
      await until(
        ownerPage,
        "the wallet says the app waits",
        () => text(ownerPage, "pending-banner"),
        (banner) => banner.includes("An app asks for access."),
      );
      await click(ownerPage, "pending-review");
      await until(
        ownerPage,
        "the consent page names the app",
        () => text(ownerPage, "consent-client"),
        (client) => client === "Budget App",
      );
      await click(ownerPage, "consent-approve");
      await until(
        ownerPage,
        "the wallet says it is approved",
        () => text(ownerPage, "consent-decided"),
        (decided) => decided.startsWith("The app may now do what it asked"),
      );

      // 4. The front door's page takes the app's browser to its finish URI,
      // once; the wallet sends no one there.
      for (let tries = 0; app.finished.length === 0 && tries < 200; tries++) {
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      const back = app.finished[0];
      assert(back, "the app's browser came back to the app");
      assertEquals(app.finished.length, 1);
      assert(appPage.url().startsWith(app.finish), appPage.url());
      assert(ownerPage.url().startsWith(wallet), ownerPage.url());
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

      // The app continues the grant, quotes for the incoming payment, and
      // pays it, all through the front door.
      const grant = await op.grant.continue({
        url: pending.continue.uri,
        accessToken: pending.continue.access_token.value,
      }, { interact_ref: reference });
      assert(isFinalizedGrantWithAccessToken(grant), "the grant is approved");
      const accessToken = grant.access_token.value;
      const quote = await op.quote.create(
        { url: payerAddress.resourceServer, accessToken },
        {
          walletAddress: payerAddress.id,
          receiver: incoming.id,
          method: "ilp",
        },
      );
      assertEquals(quote.receiveAmount.value, `${AMOUNT}`);
      const created = await op.outgoingPayment.create(
        { url: payerAddress.resourceServer, accessToken },
        { walletAddress: payerAddress.id, quoteId: quote.id },
      );
      let payment = created;
      for (let tries = 0; tries < 300; tries++) {
        payment = await op.outgoingPayment.get({
          url: created.id,
          accessToken,
        });
        if (payment.sentAmount.value === `${AMOUNT}` || payment.failed) break;
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      assertEquals(payment.failed, false);
      const receivedNow = () =>
        op.incomingPayment.get({
          url: incoming.id,
          accessToken: incomingGrant.access_token.value,
        });
      await scenario.until(
        "the incoming payment received it all",
        async () => (await receivedNow()).receivedAmount.value === `${AMOUNT}`,
      );
      assertEquals((await receivedNow()).completed, true);
    } finally {
      await browser.close();
      await proxy.close();
      await app.server.shutdown();
      restore();
      await scenario.stop();
      await rm(dir, { recursive: true, force: true }).catch(() => {});
    }
  },
);
