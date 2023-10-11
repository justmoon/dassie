import { createActor } from "@dassie/lib-reactive"

import { HandlePeerMessageActor } from "./actors/handle-peer-message"
import { SendPeerMessageActor } from "./actors/send-peer-message"
import { DiscoverNodesActor } from "./discover-nodes"
import { ForwardLinkStateUpdateActor } from "./forward-link-state-update"
import { MaintainOwnNodeTableEntryActor } from "./maintain-own-node-table-entry"
import { MaintainPeeringRelationshipsActor } from "./maintain-peering-relationships"
import { QueueBootstrapNodesActor } from "./queue-bootstrap-node"
import { RegisterPeerHttpHandlerActor } from "./register-peer-http-handler"
import { PerPeerActors } from "./run-per-peer-actors"
import { SendHeartbeatsActor } from "./send-heartbeats"

export const PeerProtocolActor = () =>
  createActor(async (sig) => {
    sig.run(HandlePeerMessageActor)
    sig.run(SendPeerMessageActor)

    // Handle incoming Dassie messages via HTTP
    sig.run(RegisterPeerHttpHandlerActor)

    await sig.run(MaintainOwnNodeTableEntryActor)
    sig.run(MaintainPeeringRelationshipsActor)

    sig.run(SendHeartbeatsActor)
    sig.run(ForwardLinkStateUpdateActor)
    sig.run(QueueBootstrapNodesActor)
    sig.run(DiscoverNodesActor)

    sig.runMap(PerPeerActors)
  })
