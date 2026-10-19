/**
 * End to end (spec/areas/testing.md#end-to-end): the reference Open
 * Payments client, `@interledger/open-payments`, gets a grant from a node,
 * creates an incoming payment there, `ilp-protocol-stream` pays it, and the
 * payment's state and the node's books are exact.
 *
 * The client is a third party with a wallet address of its own, served
 * here, whose `jwks.json` the node fetches to check its signatures. The
 * client validates every response against the Open Payments specs.
 */

import { test } from "node:test";
import { assert, assertEquals, assertRejects } from "@std/assert";
import { Buffer } from "node:buffer";
import { generateKeyPairSync } from "node:crypto";
import { readFile } from "node:fs/promises";
import { getCACertificates, setDefaultCACertificates } from "node:tls";
import WebSocket from "#ws";
import { createConnection } from "ilp-protocol-stream";
import {
  createAuthenticatedClient,
  createUnauthenticatedClient,
  isFinalizedGrantWithAccessToken,
} from "@interledger/open-payments";
import { serve } from "@dassie/dev/serve";
import { certificate, Scenario } from "@dassie/dev/scenario";

import {
  DASSIE,
  missingScenarioTools,
  SCENARIO_PORT_OFFSET,
} from "./harness.ts";
import { BtpClient } from "./btp.ts";

/** Ports of their own, beside the other suites'. */
const PORT_OFFSET = SCENARIO_PORT_OFFSET + 300;
const CLIENT_PORT = 18_960;
/** Above the node's 0.1 USD packet limit, so it takes several packets. */
const AMOUNT = 250_000_000n;

const missing = await missingScenarioTools();

/**
 * Makes `fetch` trust `ca`, until the returned function restores it. The
 * client library fetches through the global `fetch`, with Node's own
 * `Request`s, which npm undici's `fetch` (the one a CA can be passed to)
 * does not take; so it is Node's default CA list that gets `ca`.
 */
function trustFetch(ca: string): () => void {
  const original = getCACertificates("default");
  setDefaultCACertificates([...original, ca]);
  return () => setDefaultCACertificates(original);
}

/** The client's wallet address and its key set, over HTTPS. */
async function serveClient(scenario: Scenario, jwk: object) {
  const layout = scenario.options.layout;
  const tls = await certificate(
    layout.tls,
    "client",
    ["localhost"],
    layout.caRoot,
  );
  const origin = `https://localhost:${CLIENT_PORT}`;
  const server = await serve({
    port: CLIENT_PORT,
    hostname: "127.0.0.1",
    cert: tls.certificatePem,
    key: tls.keyPem,
  }, (request) => {
    const path = new URL(request.url).pathname;
    if (path === "/client/jwks.json") return Response.json({ keys: [jwk] });
    if (path === "/client") {
      return Response.json({
        id: `${origin}/client`,
        assetCode: "USD",
        assetScale: 9,
        authServer: `${origin}/auth`,
        resourceServer: origin,
      });
    }
    return new Response("Not Found", { status: 404 });
  });
  return { server, walletAddress: `${origin}/client` };
}

/**
 * Pays the incoming payment's STREAM credentials from a BTP client, giving
 * up after `timeout` ms.
 */
async function pay(
  scenario: Scenario,
  url: string,
  destination: string,
  secret: string,
  timeout = 60_000,
) {
  const ca = await readFile(scenario.caFile, "utf8");
  const plugin = new BtpClient({ server: url }, {
    WebSocket: class extends WebSocket {
      constructor(target: string) {
        super(target, { ca });
      }
    },
  });
  await plugin.connect();
  try {
    const connection = await createConnection({
      plugin,
      destinationAccount: destination,
      sharedSecret: Buffer.from(secret, "base64url"),
    });
    const stream = connection.createStream();
    await stream.sendTotal(Number(AMOUNT), { timeout });
    assertEquals(BigInt(stream.totalSent), AMOUNT);
    await connection.end();
  } finally {
    await plugin.disconnect();
  }
}

test("the reference client gets paid into an incoming payment", {
  skip: missing !== undefined,
}, async () => {
  const scenario = await Scenario.temporary("one-node", {
    dassie: DASSIE,
    portOffset: PORT_OFFSET,
  });
  const restore = trustFetch(await readFile(scenario.caFile, "utf8"));
  const { publicKey, privateKey } = generateKeyPairSync("ed25519");
  const jwk = {
    ...publicKey.export({ format: "jwk" }),
    kid: "e2e-key",
    alg: "EdDSA",
    use: "sig",
  };
  const client = await serveClient(scenario, jwk);
  try {
    const [node] = scenario.nodes;
    // `localhost`, which resolves on every system; the certificate
    // names it too.
    await node.rpc.call("setHostname", { hostname: "localhost" });
    const origin = `https://localhost:${node.httpsPort}`;
    const walletAddressUrl = `${origin}/.well-known/pay`;

    const anonymous = await createUnauthenticatedClient({});
    const walletAddress = await anonymous.walletAddress.get({
      url: walletAddressUrl,
    });
    assertEquals(walletAddress.id, walletAddressUrl);
    assertEquals(walletAddress.assetCode, "USD");
    assertEquals(walletAddress.authServer, `${origin}/gnap`);
    const keys = await anonymous.walletAddress.getKeys({
      url: walletAddressUrl,
    });
    assert(keys.keys[0].kid.startsWith("dassie-"));

    const op = await createAuthenticatedClient({
      walletAddressUrl: client.walletAddress,
      privateKey,
      keyId: "e2e-key",
    });
    const grant = await op.grant.request({ url: walletAddress.authServer }, {
      access_token: {
        access: [{
          type: "incoming-payment",
          actions: ["create", "read", "list", "complete"],
        }],
      },
    });
    assert(
      isFinalizedGrantWithAccessToken(grant),
      "the grant needs no interaction",
    );
    const accessToken = grant.access_token.value;

    const created = await op.incomingPayment.create(
      { url: walletAddress.resourceServer, accessToken },
      {
        walletAddress: walletAddressUrl,
        incomingAmount: {
          value: `${AMOUNT}`,
          assetCode: "USD",
          assetScale: 9,
        },
        metadata: { description: "end to end" },
      },
    );
    assertEquals(created.completed, false);
    assertEquals(created.receivedAmount.value, "0");
    const [method] = created.methods;
    assertEquals(method.type, "ilp");

    const token = await node.rpc.call("createBtpToken", {
      label: "payer",
      ledger: "stub+usd",
    });
    const btp = token.url.replace(`@${node.hostname}:`, "@localhost:");
    await pay(scenario, btp, method.ilpAddress, method.sharedSecret);

    const paid = await op.incomingPayment.get({ url: created.id, accessToken });
    assertEquals(paid.receivedAmount.value, `${AMOUNT}`);
    assertEquals(paid.completed, true);
    const seen = await anonymous.incomingPayment.get({ url: created.id });
    assertEquals(seen.receivedAmount?.value, `${AMOUNT}`);

    const page = await op.incomingPayment.list(
      {
        url: walletAddress.resourceServer,
        walletAddress: walletAddressUrl,
        accessToken,
      },
      { "wallet-address": walletAddressUrl, first: 10 },
    );
    assertEquals(page.result.map((payment) => payment.id), [created.id]);
    assertEquals(page.pagination.hasNextPage, false);

    // The books: the money came from the payer's token account into the
    // owner's pool, packet by packet, exactly the amount.
    const paidIn = async () => {
      const { transfers } = await node.rpc.call("listTransfers", {
        limit: 1000,
      });
      return transfers
        .filter((transfer) =>
          transfer.creditPath === "stub+usd:equity/owner" &&
          transfer.debitPath.startsWith("stub+usd:equity/owner/btp/")
        )
        .reduce((sum, transfer) => sum + transfer.amount, 0n);
    };
    assertEquals(await paidIn(), AMOUNT);

    // A second payment of the same: it is complete, so nothing gets in.
    // The node refuses every packet, which the STREAM client only reports
    // when it gives up, so it gives up after a few seconds and the books
    // show that nothing more came in.
    await assertRejects(
      () => pay(scenario, btp, method.ilpAddress, method.sharedSecret, 3_000),
    );
    assertEquals(await paidIn(), AMOUNT);

    // Another client's key with this token is refused.
    const thief = generateKeyPairSync("ed25519");
    const other = await createAuthenticatedClient({
      walletAddressUrl: client.walletAddress,
      privateKey: thief.privateKey,
      keyId: "e2e-key",
    });
    const refused = await other.incomingPayment
      .get({ url: created.id, accessToken })
      .catch((error) => error);
    assertEquals(refused.status, 401);
  } finally {
    await client.server.shutdown();
    restore();
    await scenario.stop();
  }
});
