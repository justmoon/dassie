/**
 * The Dassie core crates compiled to WASM, for scripts and tests: identity
 * derivation (`dassie-identity`), ILP packets (`dassie-ilp`) and STREAM
 * (`dassie-stream`). The same code runs in the node, so a key derived
 * here is the key the node derives.
 *
 * It is the browsers' build (`gen/web`), instantiated here at once from
 * the file next to it. Browsers load it through `./web` and `./web/stream`
 * instead, which fetch the module.
 *
 * Build the bindings first with `node --run build:wasm`.
 *
 * @module
 */

import { readFileSync } from "node:fs";
import { initSync as initIdentity } from "./gen/web/dassie_identity.js";
import { initSync as initStream } from "./gen/web/dassie_stream.js";

const wasm = (name: string) =>
  readFileSync(new URL(`./gen/web/${name}_bg.wasm`, import.meta.url));

initIdentity({ module: wasm("dassie_identity") });
initStream({ module: wasm("dassie_stream") });

export {
  deriveNodeKey,
  devSessionToken,
  generateMnemonic,
  loginSecret,
  mnemonicFromEntropy,
  nodeId,
  nodeIdFromLabel,
  nodeLabel,
  normalizeMnemonic,
  publicKey,
  sign,
  validateMnemonic,
} from "./gen/web/dassie_identity.js";

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
} from "./gen/web/dassie_stream.js";
