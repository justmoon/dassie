/**
 * The Dassie core crates for Deno, compiled to WASM: identity derivation
 * (`dassie-identity`), ILP packets (`dassie-ilp`) and STREAM
 * (`dassie-stream`). The same code runs in the node, so a key derived
 * here is the key the node derives.
 *
 * Build the bindings first with `deno task build:wasm`. Browsers load the
 * `gen/web` build instead, through `./web` and `./web/stream`.
 *
 * @module
 */

export {
  deriveNodeKey,
  devSessionToken,
  generateMnemonic,
  loginSecret,
  mnemonicFromEntropy,
  nodeId,
  normalizeMnemonic,
  publicKey,
  sign,
  validateMnemonic,
} from "./gen/deno/dassie_identity.js";

export {
  decodeAmountTooLarge,
  decodeIlpPacket,
  decodeStreamPacket,
  encodeAmountTooLarge,
  encodeFulfill,
  encodePrepare,
  encodeReject,
  IlpPacket,
  StreamConnection,
  StreamCredentials,
  StreamEvent,
  StreamFrameView,
  StreamPacketView,
  StreamServer,
  StreamTransmit,
} from "./gen/deno/dassie_stream.js";
