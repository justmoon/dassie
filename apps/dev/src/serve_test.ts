import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { serve, type Socket, upgradeWebSocket } from "./serve.ts";

const PROTOCOL = "test.v1";

function opened(socket: WebSocket): Promise<void> {
  return new Promise((resolve, reject) => {
    socket.onopen = () => resolve();
    socket.onerror = () => reject(new Error("did not open"));
  });
}

function message(socket: WebSocket): Promise<unknown> {
  return new Promise((resolve) => {
    socket.onmessage = (event) => resolve(event.data);
  });
}

test("a handler answers requests, and upgrades with the response it returns", async () => {
  const sockets: Socket[] = [];
  let states: number[] = [];
  const server = await serve({ hostname: "127.0.0.1", port: 0 }, (request) => {
    const url = new URL(request.url);
    if (url.pathname === "/plain") return new Response("hello");
    if (url.pathname === "/refused") {
      return new Response("no", { status: 403 });
    }
    const { socket, response } = upgradeWebSocket(request, {
      protocol: PROTOCOL,
    });
    sockets.push(socket);
    // Not open until the handler returns.
    states = [socket.readyState];
    socket.binaryType = "arraybuffer";
    socket.onopen = () => states.push(socket.readyState);
    socket.onmessage = (event) => {
      const bytes = new Uint8Array(event.data as ArrayBuffer);
      socket.send(bytes.map((byte) => byte + 1));
    };
    return response;
  });
  const base = `127.0.0.1:${server.addr.port}`;

  const plain = await fetch(`http://${base}/plain`);
  assertEquals(await plain.text(), "hello");

  const refused = new WebSocket(`ws://${base}/refused`);
  const failed = await opened(refused).then(() => false, () => true);
  assertEquals(failed, true);

  const client = new WebSocket(`ws://${base}/echo`, PROTOCOL);
  client.binaryType = "arraybuffer";
  await opened(client);
  assertEquals(client.protocol, PROTOCOL);
  assertEquals(states, [0, 1]);
  const reply = message(client);
  client.send(new Uint8Array([1, 2, 3]));
  assertEquals([...new Uint8Array((await reply) as ArrayBuffer)], [2, 3, 4]);

  // Shutting down does not wait for the open socket.
  const closed = new Promise<number>((resolve) => {
    client.onclose = (event) => resolve(event.code);
  });
  await server.shutdown();
  await closed;
  assertEquals(sockets[0].readyState, 3);
});
