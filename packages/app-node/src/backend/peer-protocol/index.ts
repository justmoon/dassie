import { createActor } from "@dassie/lib-reactive"

import type { PerSubnetParameters } from "../subnets/manage-subnet-instances"
import { handlePeerMessage } from "./actions/handle-peer-message"
import { sendPeerMessage } from "./actions/send-peer-message"
import { calculateRoutes } from "./calculate-routes"
import { peersArrayComputation } from "./computed/peers"
import { discoverNodes } from "./discover-nodes"
import { forwardLinkStateUpdate } from "./forward-link-state-update"
import { maintainOwnNodeTableEntry } from "./maintain-own-node-table-entry"
import { maintainPeeringRelationships } from "./maintain-peering-relationships"
import { queueBootstrapNodes } from "./queue-bootstrap-node"
import { registerPeerHttpHandler } from "./register-peer-http-handler"
import { runPerPeerEffects } from "./run-per-peer-effects"
import { sendHeartbeats } from "./send-heartbeats"

export const speakPeerProtocol = () =>
  createActor((sig) => {
    sig.run(handlePeerMessage, undefined, { register: true })
    sig.run(sendPeerMessage, undefined, { register: true })

    // Handle incoming Dassie messages via HTTP
    sig.run(registerPeerHttpHandler)

    sig.run(sendHeartbeats)
    sig.run(forwardLinkStateUpdate)
    sig.run(discoverNodes)

    sig.for(peersArrayComputation, runPerPeerEffects)
  })

/**
 * Some effects are specific to each subnet so we export this helper which is called from the subnet instantiation code.
 */
export const speakPeerProtocolPerSubnet = () =>
  createActor(async (sig, parameters: PerSubnetParameters) => {
    sig.run(calculateRoutes, parameters)
    await sig.run(maintainOwnNodeTableEntry, parameters)
    sig.run(maintainPeeringRelationships, parameters)
    sig.run(queueBootstrapNodes, parameters)
  })
