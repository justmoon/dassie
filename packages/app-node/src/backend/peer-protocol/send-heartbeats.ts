import { ActorContext, createActor } from "@dassie/lib-reactive"

import { NodeIdSignal } from "../ilp-connector/computed/node-id"
import { SendPeerMessageActor } from "./actors/send-peer-message"
import { PeersSignal } from "./computed/peers"
import {
  MAX_HEARTBEAT_INTERVAL,
  MIN_HEARTBEAT_INTERVAL,
} from "./constants/timings"
import { NodeTableStore } from "./stores/node-table"
import { NodeId } from "./types/node-id"

export const SendHeartbeatsActor = () =>
  createActor((sig) => {
    const ownNodeId = sig.reactor.use(NodeIdSignal).read()

    // Get the current peers and re-run the actor if they change
    const peers = sig.get(PeersSignal)

    const linkStateUpdate = sig.get(NodeTableStore, (nodeTable) =>
      nodeTable.get(ownNodeId),
    )?.linkState?.lastUpdate

    if (!linkStateUpdate) {
      return
    }

    // Send heartbeats to direct peers
    for (const peer of peers) {
      sendHeartbeat(sig, {
        peerNodeId: peer,
        lastLinkStateUpdate: linkStateUpdate,
      })
    }

    sig.timeout(
      sig.wake,
      MIN_HEARTBEAT_INTERVAL +
        Math.random() * (MAX_HEARTBEAT_INTERVAL - MIN_HEARTBEAT_INTERVAL),
    )
  })

interface HeartbeatParameters {
  peerNodeId: NodeId
  lastLinkStateUpdate: Uint8Array
}

const sendHeartbeat = (
  sig: ActorContext,
  { peerNodeId, lastLinkStateUpdate }: HeartbeatParameters,
) => {
  sig.reactor.use(SendPeerMessageActor).api.send.tell({
    destination: peerNodeId,
    message: {
      type: "linkStateUpdate",
      value: {
        bytes: lastLinkStateUpdate,
      },
    },
  })
}
