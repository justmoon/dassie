import { assert, assertEquals } from "@std/assert";
import { once } from "node:events";
import { type AddressInfo, createServer } from "node:net";
import { connect as connectTls } from "node:tls";
import { MAX_HELLO_BYTES, parseSni, type Sni, upstreamPort } from "./sni.ts";

function u16(n: number): number[] {
  return [n >> 8, n & 255];
}

function u24(n: number): number[] {
  return [n >> 16, (n >> 8) & 255, n & 255];
}

/** A ClientHello with `extensions`, in one record. */
function hello(extensions: number[][]): Uint8Array {
  const ext = extensions.flat();
  const body = [
    3,
    3,
    ...new Array(32).fill(7),
    0,
    ...u16(2),
    0x13,
    0x01,
    1,
    0,
    ...u16(ext.length),
    ...ext,
  ];
  const handshake = [1, ...u24(body.length), ...body];
  return new Uint8Array([22, 3, 1, ...u16(handshake.length), ...handshake]);
}

function sniExtension(name: string): number[] {
  const bytes = [...new TextEncoder().encode(name)];
  const entry = [0, ...u16(bytes.length), ...bytes];
  const list = [...u16(entry.length), ...entry];
  return [0, 0, ...u16(list.length), ...list];
}

const OTHER = [0, 43, 0, 3, 2, 3, 4]; // supported_versions

Deno.test("the name in a ClientHello", () => {
  assertEquals(parseSni(hello([OTHER, sniExtension("D3.localhost")])), {
    kind: "hello",
    name: "d3.localhost",
  });
  assertEquals(parseSni(hello([OTHER])), { kind: "hello" });
});

Deno.test("a ClientHello in pieces needs more until it is whole", () => {
  const whole = hello([sniExtension("d12.localhost"), OTHER]);
  for (let end = 0; end < whole.length; end++) {
    assertEquals(parseSni(whole.subarray(0, end)), { kind: "incomplete" });
  }
  assertEquals(parseSni(whole).kind, "hello");
});

Deno.test("what is not a ClientHello is invalid", () => {
  assertEquals(parseSni(new TextEncoder().encode("GET / HTTP/1.1\r\n")), {
    kind: "invalid",
  });
  // Too long a record.
  assertEquals(parseSni(new Uint8Array([22, 3, 1, 0x40, 0x01])), {
    kind: "invalid",
  });
  // A truncated extension.
  const broken = hello([sniExtension("d1.localhost")]);
  broken[broken.length - 13] = 0xff; // the name's length
  assertEquals(parseSni(broken).kind, "invalid");
  // A name with bytes no host name has.
  assertEquals(parseSni(hello([sniExtension("a b")])).kind, "invalid");
});

Deno.test("random bytes never throw", () => {
  let seed = 42;
  const random = () => (seed = (seed * 1103515245 + 12345) % 2 ** 31) / 2 ** 31;
  for (let round = 0; round < 2_000; round++) {
    const base = round % 2 === 0
      ? hello([sniExtension("d1.localhost"), OTHER])
      : new Uint8Array(Math.floor(random() * 200));
    const bytes = new Uint8Array(base);
    for (let i = 0; i < 4; i++) {
      bytes[Math.floor(random() * bytes.length)] = Math.floor(random() * 256);
    }
    const result = parseSni(bytes);
    assert(["incomplete", "hello", "invalid"].includes(result.kind));
  }
});

Deno.test("a real client's ClientHello", async () => {
  const server = createServer();
  server.listen({ host: "127.0.0.1", port: 0 });
  await once(server, "listening");
  const { port } = server.address() as AddressInfo;
  const received = new Promise<Sni>((resolve) => {
    server.once("connection", (socket) => {
      const buffer = new Uint8Array(MAX_HELLO_BYTES);
      let length = 0;
      socket.on("error", () => {});
      socket.on("data", (chunk: Uint8Array) => {
        const take = chunk.subarray(0, buffer.length - length);
        buffer.set(take, length);
        length += take.length;
        const result = parseSni(buffer.subarray(0, length));
        if (result.kind !== "incomplete" || length === buffer.length) {
          socket.destroy();
          resolve(result);
        }
      });
      socket.once("end", () => {
        socket.destroy();
        resolve(parseSni(buffer.subarray(0, length)));
      });
    });
  });
  const tls = connectTls({
    host: "127.0.0.1",
    port,
    servername: "d7.localhost",
  });
  tls.on("error", () => {});
  const result = await received;
  tls.destroy();
  assertEquals(result, { kind: "hello", name: "d7.localhost" });
  server.close();
});

Deno.test("names go to their node, the rest to the dashboard", () => {
  const ports = { dashboard: 10100, node: (index: number) => 5001 + index };
  assertEquals(upstreamPort("d1.localhost", ports, 100), 5001);
  assertEquals(upstreamPort("d100.localhost", ports, 100), 5100);
  assertEquals(upstreamPort("d101.localhost", ports, 100), 10100);
  assertEquals(upstreamPort("d0.localhost", ports, 100), 10100);
  assertEquals(upstreamPort("localhost", ports, 100), 10100);
  assertEquals(upstreamPort("dev-rpc.localhost", ports, 100), 10100);
  assertEquals(upstreamPort(undefined, ports, 100), 10100);
});
