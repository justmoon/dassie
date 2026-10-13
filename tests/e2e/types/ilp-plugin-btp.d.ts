// The part of `ilp-plugin-btp` 1.5 the end-to-end tests use. Its own types
// pull in @types/node 10, which clashes with Deno's.

import type { Buffer } from "node:buffer";

/** A BTP client, as an ILP plugin. */
declare class BtpPlugin {
  constructor(
    options: { server: string },
    modules?: { WebSocket?: new (url: string) => unknown },
  );
  connect(): Promise<void>;
  disconnect(): Promise<void>;
  isConnected(): boolean;
  sendData(data: Buffer): Promise<Buffer>;
  registerDataHandler(handler: (data: Buffer) => Promise<Buffer>): void;
  deregisterDataHandler(): void;
}

export default BtpPlugin;
