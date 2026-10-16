// dassie-stream (WASM) against ilp-protocol-stream, in both directions
// (spec 7 §7). The native build runs the same scenarios through
// native_interop_test.ts.

import { test } from "node:test";
import { Buffer } from "node:buffer";

import { assert, assertEquals } from "@std/assert";
import {
  type Connection,
  createConnection,
  createServer,
  type DataAndMoneyStream,
  verifyReceipt,
} from "ilp-protocol-stream";

import { StreamConnection, StreamServer } from "@dassie/identity-wasm";
import { ClientDriver, ServerDriver } from "@dassie/identity-wasm/stream";

import { createPlugin, Network } from "./network.ts";

function waitUntil(condition: () => boolean, timeoutMs = 20_000) {
  return new Promise<void>((resolve, reject) => {
    const started = Date.now();
    const check = () => {
      if (condition()) return resolve();
      if (Date.now() - started > timeoutMs) {
        return reject(new Error("condition not met in time"));
      }
      setTimeout(check, 5);
    };
    check();
  });
}

test("dassie client pays an ilp-protocol-stream server", async () => {
  const network = new Network();
  const server = await createServer({
    plugin: createPlugin(network, "test.upstream"),
    serverSecret: Buffer.alloc(32, 7),
  });
  let received = 0n;
  let data = "";
  server.on("connection", (connection: Connection) => {
    connection.on("stream", (stream: DataAndMoneyStream) => {
      stream.setReceiveMax(10_000_000);
      stream.on("money", (amount: string) => received += BigInt(amount));
      stream.on("data", (chunk: Buffer) => data += chunk.toString());
    });
  });
  const { destinationAccount, sharedSecret } = server
    .generateAddressAndSecret();

  const connection = new StreamConnection(
    new Uint8Array(sharedSecret),
    destinationAccount,
    "test.dassie",
    "USD",
    6,
  );
  const driver = new ClientDriver(connection, {
    sendPacket: (prepare) => network.route(prepare),
  });
  network.register(
    "test.dassie",
    (prepare) => Promise.resolve(driver.handlePrepare(prepare)),
  );

  assertEquals(await driver.measureExchangeRate(), [1n, 1n]);
  const result = await driver.pay(1_000_000n);
  assertEquals(result.sent, 1_000_000n);
  await waitUntil(() => received === 1_000_000n);

  const stream = connection.openStream();
  connection.write(stream, new TextEncoder().encode("hello upstream"));
  driver.pump();
  await waitUntil(() => data === "hello upstream");

  await driver.close();
  await server.close();
});

test(
  "data keeps flowing to ilp-protocol-stream after streams close",
  async () => {
    // ilp-protocol-stream counts connection-level flow control over open
    // streams only. Counting closed streams too, as dassie-stream once did,
    // stalls once 64 KiB went over streams that have since closed.
    const network = new Network();
    const server = await createServer({
      plugin: createPlugin(network, "test.upstream"),
      serverSecret: Buffer.alloc(32, 8),
    });
    const received = new Map<number, number>();
    server.on("connection", (connection: Connection) => {
      connection.on("stream", (stream: DataAndMoneyStream) => {
        stream.on("data", (chunk: Buffer) => {
          received.set(
            stream.id,
            (received.get(stream.id) ?? 0) + chunk.length,
          );
        });
      });
    });
    const { destinationAccount, sharedSecret } = server
      .generateAddressAndSecret();
    const connection = new StreamConnection(
      new Uint8Array(sharedSecret),
      destinationAccount,
      "test.dassie",
      "USD",
      6,
    );
    const driver = new ClientDriver(connection, {
      sendPacket: (prepare) => network.route(prepare),
    });
    network.register(
      "test.dassie",
      (prepare) => Promise.resolve(driver.handlePrepare(prepare)),
    );

    const size = 70_000;
    for (let round = 0; round < 2; round++) {
      const stream = connection.openStream();
      const data = new Uint8Array(size).fill(1);
      let written = 0;
      await waitUntil(() => {
        if (written < size) {
          written += connection.write(stream, data.subarray(written));
          driver.pump();
        }
        return received.get(Number(stream)) === size;
      });
      connection.closeStream(stream, 1);
      driver.pump();
    }

    await driver.close();
    await server.close();
  },
);

test("ilp-protocol-stream client pays a dassie server", async () => {
  const network = new Network();
  const server = new StreamServer(
    new Uint8Array(32).fill(3),
    "test.dassie",
    "USD",
    6,
    10_000_000n,
  );
  const driver = new ServerDriver(server);
  network.register(
    "test.dassie",
    (prepare) => Promise.resolve(driver.handlePrepare(prepare)),
  );

  const receiptNonce = new Uint8Array(16).fill(1);
  const receiptSecret = new Uint8Array(32).fill(2);
  const credentials = server.generateCredentials(
    new TextEncoder().encode("invoice-1"),
    receiptNonce,
    receiptSecret,
  );

  let closed = false;
  let connectionId: bigint | undefined;
  driver.on((id, event) => {
    connectionId = id;
    if (event.kind === "connectionClosed") closed = true;
  });

  const connection = await createConnection({
    plugin: createPlugin(network, "test.upstream"),
    destinationAccount: credentials.destination,
    sharedSecret: Buffer.from(credentials.sharedSecret),
  });

  const first = connection.createStream();
  await first.sendTotal(1000);
  const second = connection.createStream();
  await second.sendTotal(2500);

  assert(connectionId !== undefined);
  assertEquals(
    new TextDecoder().decode(server.connectionTag(connectionId)),
    "invoice-1",
  );
  assertEquals(server.totalReceived(connectionId, 1n), 1000n);
  assertEquals(server.totalReceived(connectionId, 3n), 2500n);

  // The receipts dassie-stream issued verify with the reference code.
  const receipt = verifyReceipt(second.receipt!, Buffer.from(receiptSecret));
  assertEquals(String(receipt.streamId), "3");
  assertEquals(receipt.totalReceived.toString(), "2500");

  await connection.end();
  await waitUntil(() => closed);
  driver.stop();
});
