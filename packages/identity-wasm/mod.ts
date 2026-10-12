/**
 * The Dassie core crates for Deno, compiled to WASM: identity derivation
 * (`dassie-identity`), ILP packets (`dassie-ilp`) and STREAM
 * (`dassie-stream`). The same code runs in the node, so a key derived
 * here is the key the node derives.
 *
 * Build the bindings first with `deno task build:wasm`. Browsers load the
 * `gen/web` build instead; see `tests/interop/browser_test.ts`.
 *
 * @module
 */

export {
  deriveNodeKey,
  devSessionToken,
  generateMnemonic,
  loginSecret,
  nodeId,
  normalizeMnemonic,
  publicKey,
  sign,
  validateMnemonic,
} from "./gen/deno/dassie_identity.js";

export {
  decodeIlpPacket,
  encodeFulfill,
  encodePrepare,
  encodeReject,
  IlpPacket,
  StreamConnection,
  StreamCredentials,
  StreamEvent,
  StreamServer,
  StreamTransmit,
} from "./gen/deno/dassie_stream.js";
