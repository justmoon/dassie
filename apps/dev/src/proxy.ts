/**
 * The dev environment's entry point on 443 (spec 6 §6.3): reads the name a
 * browser asks for from its ClientHello, and passes the connection on,
 * still encrypted, to that node or to the dashboard. `https://localhost`
 * is the dashboard, `https://dN.localhost` a node's wallet.
 *
 * @module
 */

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
 * Listens on loopback (IPv4, and IPv6 where there is one). Throws if the
 * port cannot be had, as 443 often cannot without privileges.
 */
export function startProxy(options: ProxyOptions): Proxy {
  const listeners = [
    Deno.listen({ hostname: "127.0.0.1", port: options.port }),
  ];
  try {
    listeners.push(Deno.listen({ hostname: "::1", port: options.port }));
  } catch {
    // No IPv6 loopback here.
  }
  let open = 0;
  for (const listener of listeners) {
    (async () => {
      for await (const conn of listener) {
        if (open >= MAX_CONNECTIONS) {
          conn.close();
          continue;
        }
        open++;
        pass(conn, options).finally(() => open--);
      }
    })().catch(() => {
      // Closed.
    });
  }
  return {
    close() {
      for (const listener of listeners) {
        try {
          listener.close();
        } catch {
          // Closed already.
        }
      }
    },
  };
}

/** The hint for a port that cannot be had. */
export function proxyHint(error: unknown, port: number): string {
  if (error instanceof Deno.errors.PermissionDenied) {
    return `cannot listen on ${port}: allow unprivileged ports with \`sudo sysctl -w net.ipv4.ip_unprivileged_port_start=0\` (Linux), or use the dashboard at its own port`;
  }
  if (error instanceof Deno.errors.AddrInUse) {
    return `port ${port} is in use: is another dev environment running?`;
  }
  return `cannot listen on ${port}: ${error}`;
}

async function pass(conn: Deno.Conn, options: ProxyOptions): Promise<void> {
  let upstream: Deno.Conn | undefined;
  try {
    const first = await readHello(conn);
    if (!first) return;
    upstream = await Deno.connect({
      hostname: "127.0.0.1",
      port: options.upstream(first.name),
    });
    await writeAll(upstream, first.bytes);
    await Promise.all([
      conn.readable.pipeTo(upstream.writable),
      upstream.readable.pipeTo(conn.writable),
    ]);
  } catch {
    // Either side went away: the browser sees the connection close, as
    // the original's proxy did for a node that is down.
  } finally {
    close(conn);
    if (upstream) close(upstream);
  }
}

/** What the client sent until its ClientHello was whole, and the name. */
async function readHello(
  conn: Deno.Conn,
): Promise<{ bytes: Uint8Array; name?: string } | undefined> {
  const buffer = new Uint8Array(MAX_HELLO_BYTES);
  let length = 0;
  const deadline = Date.now() + HELLO_TIMEOUT_MS;
  while (length < buffer.length) {
    const left = deadline - Date.now();
    if (left <= 0) return undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const timeout = new Promise<"timeout">((resolve) => {
      timer = setTimeout(() => resolve("timeout"), left);
    });
    const read = await Promise.race([
      conn.read(buffer.subarray(length)),
      timeout,
    ]);
    clearTimeout(timer);
    if (read === "timeout" || read === null) return undefined;
    length += read;
    const sni = parseSni(buffer.subarray(0, length));
    if (sni.kind === "incomplete") continue;
    return {
      bytes: buffer.slice(0, length),
      name: sni.kind === "hello" ? sni.name : undefined,
    };
  }
  return { bytes: buffer.slice(0, length) };
}

async function writeAll(conn: Deno.Conn, bytes: Uint8Array): Promise<void> {
  let written = 0;
  while (written < bytes.length) {
    written += await conn.write(bytes.subarray(written));
  }
}

function close(conn: Deno.Conn): void {
  try {
    conn.close();
  } catch {
    // Closed already, by `pipeTo`.
  }
}
