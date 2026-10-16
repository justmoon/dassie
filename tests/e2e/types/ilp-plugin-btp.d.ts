// The part of `ilp-plugin-btp` 1.5 the end-to-end tests use, which they
// import as `#ilp-plugin-btp` (see `imports` in package.json). Its own types
// pull in @types/node 10, and give its CommonJS export as `default`.

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
  // From EventEmitter2, which it extends.
  eventNames(): (string | symbol)[];
  emit(event: string, ...values: unknown[]): boolean;
}

export default BtpPlugin;
