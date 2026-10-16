import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { once } from "node:events";
import { type AddressInfo, connect, createServer, type Server } from "node:net";
import { connect as connectTls } from "node:tls";
import { startProxy } from "./proxy.ts";
import { upstreamPort } from "./sni.ts";

/** A server on a free loopback port. */
async function listening(server: Server): Promise<number> {
  server.listen({ host: "127.0.0.1", port: 0 });
  await once(server, "listening");
  return (server.address() as AddressInfo).port;
}

/** A TCP server that answers each connection with its tag and all it got. */
async function echo(tag: string): Promise<{ port: number; close(): void }> {
  const server = createServer((socket) => {
    socket.on("error", () => {});
    socket.once("data", (chunk: Uint8Array) => {
      socket.end(`${tag}:${chunk.length}`);
    });
  });
  const port = await listening(server);
  return { port, close: () => server.close() };
}

async function free(): Promise<number> {
  const server = createServer();
  const port = await listening(server);
  server.close();
  await once(server, "close");
  return port;
}

/** A ClientHello asking for `name`, as a real client sends it. */
async function clientHello(name: string): Promise<Uint8Array> {
  const server = createServer();
  const port = await listening(server);
  const received = new Promise<Uint8Array>((resolve) => {
    server.once("connection", (socket) => {
      socket.on("error", () => {});
      socket.once("data", (chunk: Uint8Array) => {
        socket.destroy();
        resolve(new Uint8Array(chunk));
      });
    });
  });
  const tls = connectTls({ host: "127.0.0.1", port, servername: name });
  tls.on("error", () => {});
  const hello = await received;
  tls.destroy();
  server.close();
  return hello;
}

async function through(port: number, hello: Uint8Array): Promise<string> {
  const socket = connect({ host: "127.0.0.1", port });
  socket.on("error", () => {});
  await once(socket, "connect");
  const answer = new Promise<string>((resolve) => {
    socket.once("data", (chunk: Uint8Array) => {
      resolve(new TextDecoder().decode(chunk));
    });
  });
  // In two pieces, as a ClientHello may arrive.
  const half = Math.floor(hello.length / 2);
  socket.write(hello.subarray(0, half));
  await new Promise((resolve) => setTimeout(resolve, 20));
  socket.write(hello.subarray(half));
  const text = await answer;
  socket.destroy();
  return text;
}

test("the proxy passes each name to its upstream, ClientHello and all", async () => {
  const dashboard = await echo("dashboard");
  const d2 = await echo("d2");
  const port = await free();
  const proxy = await startProxy({
    port,
    upstream: (name) =>
      upstreamPort(name, {
        dashboard: dashboard.port,
        node: (index) => index === 1 ? d2.port : dashboard.port,
      }, 100),
    log: () => {},
  });
  try {
    const forD2 = await clientHello("d2.localhost");
    assertEquals(await through(port, forD2), `d2:${forD2.length}`);
    const forDashboard = await clientHello("localhost");
    assertEquals(
      await through(port, forDashboard),
      `dashboard:${forDashboard.length}`,
    );
  } finally {
    proxy.close();
    dashboard.close();
    d2.close();
  }
});
