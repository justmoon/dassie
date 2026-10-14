import { assertEquals } from "@std/assert";
import { startProxy } from "./proxy.ts";
import { upstreamPort } from "./sni.ts";

/** A TCP server that answers each connection with its tag and all it got. */
function echo(tag: string): { port: number; close(): void } {
  const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
  (async () => {
    for await (const conn of listener) {
      (async () => {
        const chunk = new Uint8Array(65_536);
        const read = await conn.read(chunk);
        await conn.write(new TextEncoder().encode(`${tag}:${read}`));
        conn.close();
      })();
    }
  })().catch(() => {});
  return {
    port: (listener.addr as Deno.NetAddr).port,
    close: () => listener.close(),
  };
}

function free(): number {
  const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
  const { port } = listener.addr as Deno.NetAddr;
  listener.close();
  return port;
}

/** A ClientHello asking for `name`, as a real client sends it. */
async function clientHello(name: string): Promise<Uint8Array> {
  const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
  const { port } = listener.addr as Deno.NetAddr;
  const accepted = listener.accept();
  const tcp = await Deno.connect({ hostname: "127.0.0.1", port });
  const tls = Deno.startTls(tcp, { hostname: name });
  const conn = await accepted;
  const buffer = new Uint8Array(65_536);
  const read = await conn.read(buffer);
  conn.close();
  listener.close();
  await tls.then((t) => t.close()).catch(() => {});
  return buffer.slice(0, read ?? 0);
}

async function through(port: number, hello: Uint8Array): Promise<string> {
  const conn = await Deno.connect({ hostname: "127.0.0.1", port });
  // In two pieces, as a ClientHello may arrive.
  const half = Math.floor(hello.length / 2);
  await conn.write(hello.subarray(0, half));
  await new Promise((resolve) => setTimeout(resolve, 20));
  await conn.write(hello.subarray(half));
  const answer = new Uint8Array(256);
  const read = await conn.read(answer);
  conn.close();
  return new TextDecoder().decode(answer.subarray(0, read ?? 0));
}

Deno.test({
  name: "the proxy passes each name to its upstream, ClientHello and all",
  sanitizeOps: false,
  sanitizeResources: false,
  async fn() {
    const dashboard = echo("dashboard");
    const d2 = echo("d2");
    const port = free();
    const proxy = startProxy({
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
  },
});
