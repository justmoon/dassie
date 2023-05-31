import { createActor } from "@dassie/lib-reactive"

import type { PerSubnetParameters } from "../subnets/manage-subnet-instances"
import { handlePeerMessage } from "./actors/handle-peer-message"
import { sendPeerMessage } from "./actors/send-peer-message"
import { calculateRoutes } from "./calculate-routes"
import { peersArrayComputation } from "./computed/peers"
import { discoverNodes } from "./discover-nodes"
import { forwardLinkStateUpdate } from "./forward-link-state-update"
import { maintainOwnNodeTableEntry } from "./maintain-own-node-table-entry"
import { maintainPeeringRelationships } from "./maintain-peering-relationships"
import { queueBootstrapNodes } from "./queue-bootstrap-node"
import { registerPeerHttpHandler } from "./register-peer-http-handler"
import { runPerPeerActors } from "./run-per-peer-actors"
import { sendHeartbeats } from "./send-heartbeats"

export const speakPeerProtocol = () =>
  createActor(async (sig) => {
    sig.run(handlePeerMessage, undefined, { register: true })
    sig.run(sendPeerMessage, undefined, { register: true })

    // Handle incoming Dassie messages via HTTP
    sig.run(registerPeerHttpHandler)

    await sig.run(maintainOwnNodeTableEntry).result
    sig.run(maintainPeeringRelationships)

    sig.run(sendHeartbeats)
    sig.run(forwardLinkStateUpdate)
    sig.run(discoverNodes)
    sig.run(calculateRoutes)

    sig.for(peersArrayComputation, runPerPeerActors)
  })

/**
 * Some actors are specific to each subnet so we export this helper which is called from the subnet instantiation code.
 */
export const speakPeerProtocolPerSubnet = () =>
  createActor((sig, parameters: PerSubnetParameters) => {
    sig.run(queueBootstrapNodes, parameters)
  })
