/**
 * The dashboard's origin (spec/areas/dev.md#the-dashboards-origin): its pages,
 * its control channel at `/control`, and the relay to each node at
 * `/nodes/<dN>/rpc`, all on one origin, so the page keeps `connect-src 'self'`.
 * It listens on loopback only.
 *
 * The control channel and the relay act as the nodes' owner, so before
 * any upgrade the `Host` must be one of the dashboard's names (no rebound
 * name) and the `Origin` the page's own (no other page).
 *
 * @module
 */

import { readFile } from "node:fs/promises";
import { extname, join, normalize } from "node:path";
import type { ControlHub } from "./control.ts";
import { relay, wantsContract } from "./relay.ts";
import {
  serve,
  type Server as Listener,
  type Socket,
  upgradeWebSocket,
} from "./serve.ts";

/**
 * How long closing waits for connections to end. A dashboard that is open
 * keeps its sockets until it is told to close them, and may be about to
 * reconnect: past this, the dev server exits regardless.
 */
export const CLOSE_GRACE_MS = 1_000;

/** The names the dashboard answers to. */
export const DASHBOARD_HOSTS = ["localhost", "dev-rpc.localhost"] as const;

/** The dashboard's CSP: the wallet's, without WebAssembly. */
export const CONTENT_SECURITY_POLICY = "default-src 'self'; " +
  "script-src 'self'; style-src 'self'; img-src 'self' data:; " +
  "connect-src 'self'; object-src 'none'; base-uri 'none'; " +
  "form-action 'self'; frame-ancestors 'none'";

const TYPES: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".ico": "image/x-icon",
  ".woff2": "font/woff2",
  ".map": "application/json",
};

/** Why a request may not reach the owner's side, if it may not. */
export function refusal(
  host: string | null,
  origin: string | null,
): string | undefined {
  const name = host?.replace(/:\d{1,5}$/, "");
  if (!name || !(DASHBOARD_HOSTS as readonly string[]).includes(name)) {
    return "not a name of the dashboard";
  }
  if (origin !== `https://${host}`) return "not the dashboard's page";
  return undefined;
}

export interface ServerOptions {
  readonly ports: readonly number[];
  readonly certificatePem: string;
  readonly keyPem: string;
  /** The built dashboard. */
  readonly dashboardDir: string;
  readonly control: ControlHub;
  /** The IPC socket of node `name`, while it runs. */
  readonly nodeSocket: (name: string) => string | undefined;
}

/** A running server. */
export interface Server {
  close(): Promise<void>;
}

/** Serves on each port, at 127.0.0.1 and, where there is one, ::1. */
export async function startServer(options: ServerOptions): Promise<Server> {
  const servers: Listener[] = [];
  // Upgraded sockets, which a graceful shutdown would wait for.
  const sockets = new Set<Socket>();
  const track = (socket: Socket) => {
    sockets.add(socket);
    socket.addEventListener("close", () => sockets.delete(socket));
  };
  const handler = (request: Request) => handle(request, options, track);
  try {
    for (const port of options.ports) {
      for (const hostname of ["127.0.0.1", "::1"]) {
        try {
          servers.push(
            await serve({
              hostname,
              port,
              cert: options.certificatePem,
              key: options.keyPem,
              onError: () => new Response("internal error", { status: 500 }),
            }, handler),
          );
        } catch (error) {
          // No IPv6 loopback here; IPv4 must work.
          if (hostname === "127.0.0.1") throw error;
        }
      }
    }
  } catch (error) {
    // Those already listening would keep the process alive.
    await Promise.all(servers.map((server) => server.shutdown()));
    throw error;
  }
  return {
    async close() {
      // No new connections from here on; then the open sockets are told
      // to close.
      const shutdown = Promise.all(servers.map((server) => server.shutdown()));
      for (const socket of sockets) {
        try {
          socket.close(1001, "shutting down");
        } catch {
          // Not open yet: the shutdown ends it.
        }
      }
      sockets.clear();
      let timer: ReturnType<typeof setTimeout> | undefined;
      const late = new Promise<void>((resolve) => {
        timer = setTimeout(resolve, CLOSE_GRACE_MS);
      });
      await Promise.race([shutdown, late]);
      clearTimeout(timer);
    },
  };
}

async function handle(
  request: Request,
  options: ServerOptions,
  track: (socket: Socket) => void,
): Promise<Response> {
  const url = new URL(request.url);
  // From `Host`, or `:authority` over HTTP/2, which has no `Host`.
  const host = url.host;
  const name = host?.replace(/:\d{1,5}$/, "");
  if (!name || !(DASHBOARD_HOSTS as readonly string[]).includes(name)) {
    return new Response("misdirected", { status: 421 });
  }
  if (request.headers.get("upgrade")?.toLowerCase() === "websocket") {
    const refused = refusal(host, request.headers.get("origin"));
    if (refused) return new Response(refused, { status: 403 });
    if (url.pathname === "/control") {
      const { socket, response } = upgradeWebSocket(request);
      track(socket);
      options.control.attach(socket);
      return response;
    }
    const match = /^\/nodes\/(d[1-9]\d{0,3})\/rpc$/.exec(url.pathname);
    const socket = match ? options.nodeSocket(match[1]) : undefined;
    if (!socket) return new Response("no such node", { status: 404 });
    if (!wantsContract(request)) {
      return new Response("the contract's subprotocol only", { status: 400 });
    }
    return await relay(request, socket, track);
  }
  if (request.method !== "GET" && request.method !== "HEAD") {
    return new Response("method not allowed", { status: 405 });
  }
  return await serveFile(options.dashboardDir, url.pathname);
}

/** A file of the built dashboard, or its page for any route. */
async function serveFile(root: string, pathname: string): Promise<Response> {
  const headers = {
    "content-security-policy": CONTENT_SECURITY_POLICY,
    "cache-control": "no-cache",
    "x-content-type-options": "nosniff",
  };
  let decoded: string;
  try {
    decoded = decodeURIComponent(pathname);
  } catch {
    return new Response("bad path", { status: 400, headers });
  }
  const relative = normalize(decoded).replace(/^\/+/, "");
  if (relative.split("/").includes("..")) {
    return new Response("bad path", { status: 400, headers });
  }
  const extension = extname(relative);
  // A route of the page: its index. A missing asset: 404.
  const file = extension === "" ? "index.html" : relative;
  try {
    const body = await readFile(join(root, file));
    return new Response(body, {
      headers: {
        ...headers,
        "content-type": TYPES[extname(file)] ?? "application/octet-stream",
      },
    });
  } catch {
    if (extension !== "") {
      return new Response("not found", { status: 404, headers });
    }
    return new Response("the dashboard is being built; reload in a moment", {
      status: 503,
      headers,
    });
  }
}
