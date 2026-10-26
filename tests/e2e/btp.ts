/**
 * The reference BTP client, `ilp-plugin-btp`, as the tests use it.
 *
 * `ilp-protocol-stream` sends a last packet or two as it closes a
 * connection, and disconnects the plugin without waiting for their
 * answers, or sends after it disconnected. `ilp-plugin-btp` waits 35 s for
 * an answer that cannot come; that timer keeps the test file's process
 * alive. This client answers those calls itself once it is disconnected,
 * with an ILP Reject, as a connector would for a peer that is gone. A final
 * one (F02): ilp-protocol-stream sends a packet a temporary one rejected
 * again, without end.
 *
 * A Reject, not a failed call: whoever sent the packet may no longer be
 * listening, and a promise rejected with nobody to handle it fails the
 * test.
 */

import { Buffer } from "node:buffer";
import BtpPlugin from "#ilp-plugin-btp";
import { encodeReject } from "@dassie/wasm";

/** BTP's Response packet type, which answers a call. */
const TYPE_RESPONSE = 1;
/** The event `ilp-plugin-btp` answers a call with, by its request ID. */
const CALLBACK_EVENT = "__callback_";

/** The ILP Reject every call gets once the plugin is disconnected. */
function disconnected(): Buffer {
  return Buffer.from(
    encodeReject("F02", "", "the BTP plugin disconnected", new Uint8Array()),
  );
}

export class BtpClient extends BtpPlugin {
  override sendData(data: Buffer): Promise<Buffer> {
    if (!this.isConnected()) return Promise.resolve(disconnected());
    return super.sendData(data);
  }

  override async disconnect(): Promise<void> {
    await super.disconnect();
    const answer = {
      protocolData: [
        { protocolName: "ilp", contentType: 0, data: disconnected() },
      ],
    };
    for (const event of this.eventNames()) {
      if (typeof event === "string" && event.startsWith(CALLBACK_EVENT)) {
        this.emit(event, TYPE_RESPONSE, answer);
      }
    }
  }
}
