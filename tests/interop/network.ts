// An in-memory connector for interop tests: routes ILP packets by address
// between endpoints, answers ILDCP for each plugin, and adapts the plugin
// interface that ilp-protocol-stream expects (modelled on the original's
// lib-protocol-stream/src/test/mocks/compatibility.ts).

import { Buffer } from "node:buffer";

import { decodeIlpPacket, encodeFulfill, encodeReject } from "@dassie/wasm";

/** Answers a Prepare with a Fulfill or Reject. */
export type Handler = (prepare: Uint8Array) => Promise<Uint8Array>;

/** Routes packets to the handler with the longest matching prefix. */
export class Network {
  readonly #routes = new Map<string, Handler>();

  register(address: string, handler: Handler): () => void {
    this.#routes.set(address, handler);
    return () => this.#routes.delete(address);
  }

  async route(prepare: Uint8Array): Promise<Uint8Array> {
    const destination = decodeIlpPacket(prepare).address;
    let best: [string, Handler] | undefined;
    for (const [prefix, handler] of this.#routes) {
      const matches = destination === prefix ||
        destination.startsWith(`${prefix}.`);
      if (matches && (!best || prefix.length > best[0].length)) {
        best = [prefix, handler];
      }
    }
    if (!best) {
      return encodeReject(
        "F02",
        "test.connector",
        "no route",
        new Uint8Array(),
      );
    }
    return await best[1](prepare);
  }
}

/** The ILDCP response a parent gives `address` (RFC 31, short form). */
function ildcpResponse(address: string, code: string, scale: number) {
  const text = new TextEncoder();
  const encodedAddress = text.encode(address);
  const encodedCode = text.encode(code);
  if (encodedAddress.length > 127 || encodedCode.length > 127) {
    throw new Error("test addresses and codes are short");
  }
  return Uint8Array.from([
    encodedAddress.length,
    ...encodedAddress,
    scale,
    encodedCode.length,
    ...encodedCode,
  ]);
}

/** The plugin interface of ilp-protocol-stream, connected to `network`. */
export function createPlugin(
  network: Network,
  address: string,
  asset: { code: string; scale: number } = { code: "USD", scale: 6 },
) {
  let unregister: (() => void) | undefined;
  let connected = false;
  return {
    connect: () => {
      connected = true;
      return Promise.resolve();
    },
    disconnect: () => {
      connected = false;
      return Promise.resolve();
    },
    isConnected: () => connected,
    async sendData(data: Buffer): Promise<Buffer> {
      const prepare = new Uint8Array(data);
      if (decodeIlpPacket(prepare).address === "peer.config") {
        const fulfill = encodeFulfill(
          new Uint8Array(32),
          ildcpResponse(address, asset.code, asset.scale),
        );
        return Buffer.from(fulfill);
      }
      return Buffer.from(await network.route(prepare));
    },
    registerDataHandler(handler: (data: Buffer) => Promise<Buffer>) {
      unregister = network.register(
        address,
        async (prepare) => new Uint8Array(await handler(Buffer.from(prepare))),
      );
    },
    deregisterDataHandler() {
      unregister?.();
      unregister = undefined;
    },
  };
}
