import { createActor } from "@dassie/lib-reactive"

import { NodeId } from "../peer-protocol/types/node-id"
import { SendOutgoingSettlementsActor } from "./send-outgoing-settlements"

/**
 * Some actors are specific to each peer so we export this helper which is called from the per-peer code.
 */
export const SettlePerPeerActor = () =>
  createActor((sig, peerId: NodeId) => {
    sig.run(SendOutgoingSettlementsActor, peerId)
  })
