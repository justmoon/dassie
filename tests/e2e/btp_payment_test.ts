/**
 * The M3 done criterion (spec 7 §8): a BTP client pays the node's STREAM
 * receiver, and the journal matches the packet trace. Since M6 the client
 * pays an incoming payment, whose credentials the owner creates.
 *
 * The client is the JavaScript reference stack: `ilp-plugin-btp` connects
 * with a token the node issued, and `ilp-protocol-stream` learns its
 * address through the node's ILDCP, measures the rate, and pays. The node
 * is the real binary, set up without a browser: the owner's calls go over
 * the IPC socket, and setup over HTTPS with the key derived in WASM.
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { Buffer } from "node:buffer";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
// @ts-types="./types/ilp-plugin-btp.d.ts"
import BtpPlugin from "ilp-plugin-btp";
// @ts-types="./types/ws.d.ts"
import WebSocket from "ws";
import { createConnection } from "ilp-protocol-stream";
import { RpcClient } from "@dassie/rpc";
import { ipcConnector } from "@dassie/rpc/ipc";
import {
  type Packet,
  PacketState,
  type Transfer,
  TransferState,
} from "@dassie/rpc/proto";
import { deriveNodeKey, loginSecret } from "@dassie/identity-wasm";

import {
  DASSIE,
  missingBinary,
  PEBBLE_DIR,
  Process,
  waitForPort,
  waitForSocket,
} from "./harness.ts";

const HTTP_PORT = 18882;
const HTTPS_PORT = 18845;
/** Above the node's 0.1 USD packet limit, so the payment is split. */
const PAYMENT = 250_000_000n;
const PHRASE =
  "pulp junk march life divorce celery flower bar one angry claw crush";

/** Pebble's test root and its leaf for localhost, which the node serves. */
const ROOT = await readFile(`${PEBBLE_DIR}/root.pem`, "utf8");
const CERTIFICATE = await readFile(`${PEBBLE_DIR}/cert.pem`, "utf8");
const PRIVATE_KEY = await readFile(`${PEBBLE_DIR}/key.pem`, "utf8");

const missing = await missingBinary();

/** A `ws` socket that trusts the test root. */
class TrustingWebSocket extends WebSocket {
  constructor(url: string) {
    super(url, { ca: ROOT });
  }
}

function startDaemon(dir: string): Process {
  return new Process("dassie", DASSIE, [
    "daemon",
    "--http-port",
    `${HTTP_PORT}`,
    "--https-port",
    `${HTTPS_PORT}`,
  ], {
    env: {
      DASSIE_STATE_DIRECTORY: `${dir}/state`,
      DASSIE_IPC_SOCKET_PATH: `${dir}/ipc.sock`,
    },
  });
}

/** Calls until the ledger's engine answers. */
async function waitForLedger(owner: RpcClient) {
  const deadline = Date.now() + 15_000;
  for (;;) {
    try {
      return await owner.call("listLedgers", {});
    } catch (error) {
      if (Date.now() > deadline) throw error;
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
  }
}

/** Serves the test certificate and claims the node with `PHRASE`. */
async function setUp(owner: RpcClient) {
  await owner.call("setCertificate", {
    certificateChainPem: CERTIFICATE,
    privateKeyPem: PRIVATE_KEY,
  });
  const { url } = await owner.call("getSetupUrl", {});
  const token = url.split("/setup/")[1];
  const key = deriveNodeKey(PHRASE);
  const client = Deno.createHttpClient({ caCerts: [ROOT] });
  const origin = `https://localhost:${HTTPS_PORT}`;
  const response = await fetch(`${origin}/api/setup`, {
    method: "POST",
    client,
    headers: { "content-type": "application/json", origin },
    body: JSON.stringify({
      setupAuthorizationToken: token,
      rawDassieKeyHex: key.toHex(),
      loginAuthorizationSignature: loginSecret(key).toHex(),
    }),
  });
  assertEquals(response.status, 200, await response.text());
  client.close();
}

/** Pays an incoming payment through a BTP connection with `url`. */
async function pay(owner: RpcClient, url: string) {
  // Open Payments' URLs need a hostname.
  await owner.call("setHostname", { hostname: "localhost" });
  const { payment } = await owner.call("createIncomingPayment", {});
  assert(payment);
  const credentials = payment;
  const plugin = new BtpPlugin({ server: url }, {
    WebSocket: TrustingWebSocket,
  });
  await plugin.connect();
  try {
    const connection = await createConnection({
      plugin,
      destinationAccount: credentials.destinationAccount,
      sharedSecret: Buffer.from(credentials.sharedSecret),
    });
    const stream = connection.createStream();
    await stream.sendTotal(Number(PAYMENT));
    assertEquals(BigInt(stream.totalSent), PAYMENT);
    await connection.end();
  } finally {
    // The plugin would otherwise reconnect by itself.
    await plugin.disconnect();
  }
}

/** The whole journal, in small pages, as a client that follows it reads. */
async function journal(owner: RpcClient): Promise<Transfer[]> {
  const rows: Transfer[] = [];
  for (;;) {
    const afterSequence = rows.at(-1)?.sequence;
    const page = await owner.call("listTransfers", { afterSequence, limit: 7 });
    if (page.transfers.length === 0) return rows;
    rows.push(...page.transfers);
  }
}

/** The packet trace, in small pages. */
async function trace(owner: RpcClient): Promise<Packet[]> {
  const packets: Packet[] = [];
  for (;;) {
    const after = packets.at(-1)?.trace;
    const page = await owner.call("listPackets", { after, limit: 7 });
    if (page.packets.length === 0) return packets;
    packets.push(...page.packets);
  }
}

/**
 * Every fulfilled packet with money has exactly its posted transfer, from
 * the token's sub-account to the owner's pool; rejected packets have none;
 * nothing else is a packet transfer. Returns the total.
 */
async function journalMatchesTrace(
  owner: RpcClient,
  tokenId: bigint,
): Promise<bigint> {
  const transfers = await journal(owner);
  const packets = await trace(owner);
  const byId = new Map<bigint, Transfer>(transfers.map((t) => [t.id, t]));
  let total = 0n;
  let fulfilled = 0;
  let rejectedTooLarge = 0;
  for (const packet of packets) {
    assert(packet.state !== PacketState.PENDING, "a packet in flight");
    if (packet.state === PacketState.REJECTED) {
      if (packet.rejectCode === "F08") rejectedTooLarge++;
      for (const id of packet.transfers) {
        assert(!byId.has(id), "a rejected packet left a journal row");
      }
      continue;
    }
    if (packet.amountIn === 0n) continue;
    assertEquals(packet.transfers.length, 1);
    const transfer = byId.get(packet.transfers[0])!;
    assertEquals(transfer.state, TransferState.POSTED);
    assertEquals(transfer.amount, packet.amountIn);
    assertEquals(transfer.debitPath, `stub+usd:equity/owner/btp/${tokenId}`);
    assertEquals(transfer.creditPath, "stub+usd:equity/owner");
    assertEquals(transfer.reference.case, "packet");
    assertEquals(transfer.reference.value, packet.trace);
    total += transfer.amount;
    fulfilled++;
  }
  const packetRows = transfers.filter((t) => t.reference.case === "packet");
  assertEquals(packetRows.length, fulfilled, "a packet row without a packet");
  assert(fulfilled >= 3, "the payment was split into packets");
  assert(rejectedTooLarge > 0, "rate probes above the limit got F08");
  return total;
}

test("a BTP client pays the node's STREAM receiver, and the journal matches", {
  skip: missing !== undefined,
}, async () => {
  const dir = await mkdtemp(join(tmpdir(), "dassie-e2e-btp-"));
  let daemon = startDaemon(dir);
  const connect = ipcConnector(`${dir}/ipc.sock`);
  let owner: RpcClient | undefined;
  let passed = false;
  try {
    await waitForPort(HTTPS_PORT);
    await waitForSocket(`${dir}/ipc.sock`);
    owner = new RpcClient({ url: "ipc", connect });
    await setUp(owner);
    await waitForLedger(owner);
    const added = await owner.call("addSettlementScheme", {
      id: "stub",
      config: "",
    });
    assertEquals(added.ledger?.ownerBalance, 100_000_000_000n);
    const token = await owner.call("createBtpToken", {
      label: "e2e",
      ledger: "stub+usd",
    });
    assertEquals(
      token.url,
      `btp+wss://:${token.secret}@localhost:${HTTPS_PORT}/btp`,
    );
    const tokenId = token.token!.id;

    await pay(owner, token.url);

    assertEquals(await journalMatchesTrace(owner, tokenId), PAYMENT);
    const [ledger] = (await owner.call("listLedgers", {})).ledgers;
    // Within the owner's group: the total is unchanged.
    assertEquals(ledger.ownerBalance, 100_000_000_000n);
    assertEquals(ledger.ownerPending, 0n);
    const { tokens } = await owner.call("listBtpTokens", {});
    assertEquals(tokens[0].netReceived, -PAYMENT);
    const { accounts } = await owner.call("listAccounts", {});
    const pool = accounts.find((a) => a.path === "stub+usd:equity/owner")!;
    assertEquals(
      pool.creditsPosted - pool.debitsPosted,
      100_000_000_000n + PAYMENT,
    );
    const books = await journal(owner);

    // A restart keeps the books and deposits nothing again (ACC-6).
    owner.close();
    await daemon.stop();
    daemon = startDaemon(dir);
    await waitForPort(HTTPS_PORT);
    await waitForSocket(`${dir}/ipc.sock`);
    owner = new RpcClient({ url: "ipc", connect });
    const [restarted] = (await waitForLedger(owner)).ledgers;
    assertEquals(restarted, ledger);
    assertEquals(await journal(owner), books);
    passed = true;
  } finally {
    owner?.close();
    const { code, output } = await daemon.stop();
    // What the node said, to tell why a test failed.
    if (code !== 0 || !passed) console.error(output);
    await rm(dir, { recursive: true, force: true });
  }
});
