import { createComputed } from "@dassie/lib-reactive"

import { getPublicKey } from "../ed25519"
import { nodePrivateKeySignal } from "./node-private-key"

export const nodePublicKeySignal = () =>
  createComputed((sig) => getPublicKey(sig.get(nodePrivateKeySignal)))
