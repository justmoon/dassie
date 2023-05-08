import { createActor } from "@dassie/lib-reactive"

import type { PerPeerParameters } from "../peer-protocol/run-per-peer-actors"
import { sendOutgoingSettlements } from "./send-outgoing-settlements"

/**
 * Some actors are specific to each subnet so we export this helper which is called from the subnet instantiation code.
 */
export const settlePerPeer = () =>
  createActor((sig, parameters: PerPeerParameters) => {
    sig.run(sendOutgoingSettlements, parameters)
  })
