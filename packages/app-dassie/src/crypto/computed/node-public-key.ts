import { type Reactor, createComputed } from "@dassie/lib-reactive"

import { getPublicKey } from "../ed25519"
import { NodePrivateKeySignal } from "./node-private-key"

export const NodePublicKeySignal = (reactor: Reactor) =>
  createComputed(reactor, (sig) =>
    getPublicKey(sig.readAndTrack(NodePrivateKeySignal)),
  )
