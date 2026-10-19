/**
 * The dev environment's entry point on 443 (spec/areas/dev.md#sni-proxy): reads
 * the name a browser asks for from its ClientHello, and passes the connection
 * on, still encrypted, to that node or to the dashboard. `https://localhost` is
 * the dashboard, `https://dN.localhost` a node's wallet.
 *
 * @module
 */

import { connect, createServer, type Server, type Socket } from "node:net";
import { MAX_HELLO_BYTES, parseSni } from "./sni.ts";

/** How long a client has to send its ClientHello. */
const HELLO_TIMEOUT_MS = 5_000;
/** Connections passed on at once, at most. */
const MAX_CONNECTIONS = 256;

export interface ProxyOptions {
  readonly port: number;
  /** The port for a name, or for no name. */
  readonly upstream: (name: string | undefined) => number;
  readonly log: (message: string) => void;
}

/** A running proxy. */
export interface Proxy {
  close(): void;
}

/**
 * Listens on loopback (IPv4, and IPv6 where there is one). Rejects if the
 * port cannot be had, as 443 often cannot without privileges.
 */
export async function startProxy(options: ProxyOptions): Promise<Proxy> {
  let open = 0;
  const accept = (socket: Socket) => {
    if (open >= MAX_CONNECTIONS) {
      socket.destroy();
      return;
    }
    open++;
    socket.once("close", () => open--);
    void pass(socket, options);
  };
  const servers = [await listen("127.0.0.1", options.port, accept)];
  try {
    servers.push(await listen("::1", options.port, accept));
  } catch {
    // No IPv6 loopback here.
  }
  return {
    close() {
      for (const server of servers) server.close();
    },
  };
}

function listen(
  host: string,
  port: number,
  accept: (socket: Socket) => void,
): Promise<Server> {
  return new Promise((resolve, reject) => {
    // Half-open: each direction is passed on, and ended, by itself.
    const server = createServer({ allowHalfOpen: true }, accept);
    server.once("error", reject);
    server.listen({ host, port }, () => {
      server.off("error", reject);
      server.on("error", () => {});
      resolve(server);
    });
  });
}

/** The hint for a port that cannot be had. */
export function proxyHint(error: unknown, port: number): string {
  const code = (error as NodeJS.ErrnoException | undefined)?.code;
  if (code === "EACCES") {
    return `cannot listen on ${port}: allow unprivileged ports with \`sudo sysctl -w net.ipv4.ip_unprivileged_port_start=0\` (Linux), or use the dashboard at its own port`;
  }
  if (code === "EADDRINUSE") {
    return `port ${port} is in use: is another dev environment running?`;
  }
  return `cannot listen on ${port}: ${error}`;
}

async function pass(socket: Socket, options: ProxyOptions): Promise<void> {
  // Either side going away closes both: the browser sees the connection
  // close, as the original's proxy did for a node that is down.
  socket.on("error", () => {});
  const first = await readHello(socket);
  if (!first || socket.destroyed) {
    socket.destroy();
    return;
  }
  const upstream = connect({
    host: "127.0.0.1",
    port: options.upstream(first.name),
    allowHalfOpen: true,
  });
  upstream.on("error", () => {});
  upstream.once("close", () => socket.destroy());
  socket.once("close", () => upstream.destroy());
  upstream.write(first.bytes);
  socket.pipe(upstream);
  upstream.pipe(socket);
}

/**
 * What the client sent until its ClientHello was whole, and the name. The
 * socket is paused after it, for the pipe to take over.
 */
function readHello(
  socket: Socket,
): Promise<{ bytes: Uint8Array; name?: string } | undefined> {
  return new Promise((resolve) => {
    let received = new Uint8Array(0);
    const done = (hello?: { bytes: Uint8Array; name?: string }) => {
      clearTimeout(timer);
      socket.pause();
      socket.off("data", onData);
      socket.off("end", gone);
      socket.off("close", gone);
      resolve(hello);
    };
    const gone = () => done();
    const onData = (chunk: Uint8Array) => {
      const more = new Uint8Array(received.length + chunk.length);
      more.set(received);
      more.set(chunk, received.length);
      received = more;
      const sni = parseSni(received.subarray(0, MAX_HELLO_BYTES));
      if (sni.kind === "incomplete" && received.length < MAX_HELLO_BYTES) {
        return;
      }
      done({
        bytes: received,
        name: sni.kind === "hello" ? sni.name : undefined,
      });
    };
    const timer = setTimeout(gone, HELLO_TIMEOUT_MS);
    socket.on("data", onData);
    socket.once("end", gone);
    socket.once("close", gone);
  });
}
