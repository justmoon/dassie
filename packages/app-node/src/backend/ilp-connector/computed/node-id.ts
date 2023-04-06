import { createComputed } from "@dassie/lib-reactive"

import { nodePublicKeySignal } from "../../crypto/computed/node-public-key"
import { calculateNodeId } from "../utils/calculate-node-id"

export const nodeIdSignal = () =>
  createComputed((sig) => calculateNodeId(sig.get(nodePublicKeySignal)))
