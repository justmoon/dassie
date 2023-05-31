import { createActor } from "@dassie/lib-reactive"

import { NodeId } from "../peer-protocol/types/node-id"
import { sendOutgoingSettlements } from "./send-outgoing-settlements"

/**
 * Some actors are specific to each subnet so we export this helper which is called from the subnet instantiation code.
 */
export const settlePerPeer = () =>
  createActor((sig, peerId: NodeId) => {
    sig.run(sendOutgoingSettlements, peerId)
  })
