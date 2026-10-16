import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer, type Socket as NetSocket } from "node:net";
import { tmpdir } from "node:os";
import { SUBPROTOCOL } from "@dassie/rpc";
import { MAX_MESSAGE_BYTES, relay } from "./relay.ts";
import { serve } from "./serve.ts";

/** A node's IPC socket that echoes every frame. */
async function echoNode(path: string) {
  const connections = new Set<NetSocket>();
  const server = createServer((connection) => {
    connections.add(connection);
    connection.once("close", () => connections.delete(connection));
    let buffered = Buffer.alloc(0);
    connection.on("data", (chunk: Buffer) => {
      buffered = Buffer.concat([buffered, chunk]);
      while (buffered.length >= 4) {
        const length = buffered.readUInt32BE(0);
        if (buffered.length < 4 + length) break;
        connection.write(buffered.subarray(0, 4 + length));
        buffered = buffered.subarray(4 + length);
      }
    });
  });
  await new Promise<void>((resolve) => server.listen(path, resolve));
  return {
    close() {
      for (const connection of connections) connection.destroy();
      return new Promise<void>((resolve) => server.close(() => resolve()));
    },
  };
}

function opened(socket: WebSocket): Promise<void> {
  return new Promise((resolve, reject) => {
    socket.onopen = () => resolve();
    socket.onerror = () => reject(new Error("did not open"));
  });
}

function closed(socket: WebSocket): Promise<number> {
  return new Promise((resolve) => {
    socket.onclose = (event) => resolve(event.code);
  });
}

test("the relay passes messages both ways, and closes on a message too large or not binary", async () => {
  const dir = await mkdtemp(`${tmpdir()}/dassie-relay-`);
  const path = `${dir}/ipc.sock`;
  const node = await echoNode(path);
  const server = await serve(
    { hostname: "127.0.0.1", port: 0 },
    (request) => relay(request, path),
  );
  const url = `ws://127.0.0.1:${server.addr.port}/nodes/d1/rpc`;
  const connect = async () => {
    const socket = new WebSocket(url, SUBPROTOCOL);
    socket.binaryType = "arraybuffer";
    await opened(socket);
    return socket;
  };
  try {
    const client = await connect();
    assertEquals(client.protocol, SUBPROTOCOL);
    const echoed = new Promise<ArrayBuffer>((resolve) => {
      client.onmessage = (event) => resolve(event.data as ArrayBuffer);
    });
    client.send(new Uint8Array([1, 2, 3]));
    assertEquals([...new Uint8Array(await echoed)], [1, 2, 3]);
    // The limit itself still passes.
    const large = new Promise<number>((resolve) => {
      client.onmessage = (event) =>
        resolve((event.data as ArrayBuffer).byteLength);
    });
    client.send(new Uint8Array(MAX_MESSAGE_BYTES));
    assertEquals(await large, MAX_MESSAGE_BYTES);

    const tooLarge = closed(client);
    client.send(new Uint8Array(MAX_MESSAGE_BYTES + 1));
    assertEquals(await tooLarge, 1009);

    const text = await connect();
    const notBinary = closed(text);
    text.send("hello");
    assertEquals(await notBinary, 1003);
  } finally {
    await server.shutdown();
    await node.close();
    await rm(dir, { recursive: true, force: true });
  }
});
