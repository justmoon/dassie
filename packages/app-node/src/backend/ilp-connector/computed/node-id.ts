import { type Reactor, createComputed } from "@dassie/lib-reactive"

import { NodePublicKeySignal } from "../../crypto/computed/node-public-key"
import { calculateNodeId } from "../utils/calculate-node-id"

export const NodeIdSignal = (reactor: Reactor) =>
  createComputed(reactor, (sig) =>
    calculateNodeId(sig.readAndTrack(NodePublicKeySignal)),
  )
