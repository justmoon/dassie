/**
 * STREAM and ILP packets (`dassie-stream`, `dassie-ilp`) for browsers: the
 * `gen/web` build, handed out by {@link loadStream} once its WASM module is
 * instantiated, as {@link loadIdentity} does for identity derivation. The
 * website's STREAM toy runs on it.
 *
 * Build the bindings first with `deno task build:wasm`.
 *
 * @module
 */

import init, {
  decodeAmountTooLarge,
  decodeIlpPacket,
  decodeStreamPacket,
  encodeAmountTooLarge,
  encodeFulfill,
  encodePrepare,
  encodeReject,
  StreamConnection,
  StreamServer,
} from "./gen/web/dassie_stream.js";

/** The classes and functions of the STREAM module. */
export interface Stream {
  StreamConnection: typeof StreamConnection;
  StreamServer: typeof StreamServer;
  decodeIlpPacket: typeof decodeIlpPacket;
  decodeStreamPacket: typeof decodeStreamPacket;
  decodeAmountTooLarge: typeof decodeAmountTooLarge;
  encodeAmountTooLarge: typeof encodeAmountTooLarge;
  encodePrepare: typeof encodePrepare;
  encodeFulfill: typeof encodeFulfill;
  encodeReject: typeof encodeReject;
}

let loading: Promise<Stream> | undefined;

/**
 * Instantiates the WASM module, once per page, and returns its classes and
 * functions. A failed load is not remembered, so a later call tries again.
 */
export function loadStream(): Promise<Stream> {
  loading ??= init().then(
    (): Stream => ({
      StreamConnection,
      StreamServer,
      decodeIlpPacket,
      decodeStreamPacket,
      decodeAmountTooLarge,
      encodeAmountTooLarge,
      encodePrepare,
      encodeFulfill,
      encodeReject,
    }),
    (error: unknown) => {
      loading = undefined;
      throw error;
    },
  );
  return loading;
}
