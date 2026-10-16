/**
 * The reference BTP client, `ilp-plugin-btp`, as the tests use it.
 *
 * `ilp-protocol-stream` sends a last packet or two as it closes a
 * connection, and disconnects the plugin without waiting for their
 * answers, or sends after it disconnected. `ilp-plugin-btp` waits 35 s for
 * an answer that cannot come; that timer keeps the test file's process
 * alive. This client fails those calls once it is disconnected, as the
 * timer would have in the end.
 */

import type { Buffer } from "node:buffer";
import BtpPlugin from "#ilp-plugin-btp";

/** BTP's Error packet type, which fails a call. */
const TYPE_ERROR = 2;
/** The event `ilp-plugin-btp` answers a call with, by its request ID. */
const CALLBACK_EVENT = "__callback_";

export class BtpClient extends BtpPlugin {
  override sendData(data: Buffer): Promise<Buffer> {
    if (!this.isConnected()) {
      return Promise.reject(new Error("the BTP plugin is disconnected"));
    }
    return super.sendData(data);
  }

  override async disconnect(): Promise<void> {
    await super.disconnect();
    for (const event of this.eventNames()) {
      if (typeof event === "string" && event.startsWith(CALLBACK_EVENT)) {
        this.emit(event, TYPE_ERROR, { name: "the BTP plugin disconnected" });
      }
    }
  }
}
