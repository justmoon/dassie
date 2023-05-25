import assert from "node:assert"

import { createLogger } from "@dassie/lib-logger"
import { ActorContext, createActor } from "@dassie/lib-reactive"

import { nodeIdSignal } from "../ilp-connector/computed/node-id"
import { sendPeerMessage } from "./actors/send-peer-message"
import { peersComputation } from "./computed/peers"
import { requestedPeersComputation } from "./computed/requested-peers"
import { nodeTableStore } from "./stores/node-table"
import { NodeId } from "./types/node-id"
import { SubnetId } from "./types/subnet-id"

const logger = createLogger("das:node:peer-greeter")

const MAX_HEARTBEAT_INTERVAL = 20_000

export const sendHeartbeats = () =>
  createActor((sig) => {
    const ownNodeId = sig.use(nodeIdSignal).read()

    // Get the current peers and re-run the actor if they change
    const peers = sig.get(peersComputation)
    const requestedPeers = sig.get(requestedPeersComputation)

    const linkStateUpdate = sig.get(nodeTableStore, (nodeTable) =>
      nodeTable.get(ownNodeId)
    )?.linkState.lastUpdate

    if (!linkStateUpdate) {
      return
    }

    // Send peer requests
    for (const peer of requestedPeers) {
      const peerState = sig.use(nodeTableStore).read().get(peer)?.peerState

      assert(peerState?.id === "request-peering")

      sendPeeringRequest(sig, {
        peerNodeId: peer,
        subnetId: peerState.subnetId,
        lastLinkStateUpdate: linkStateUpdate,
      })
    }

    // Send heartbeats to existing peers
    for (const peer of peers) {
      sendHeartbeat(sig, {
        peerNodeId: peer,
        lastLinkStateUpdate: linkStateUpdate,
      })
    }

    sig.timeout(sig.wake, Math.random() * MAX_HEARTBEAT_INTERVAL)
  })

interface PeeringRequestParameters {
  peerNodeId: NodeId
  subnetId: SubnetId
  lastLinkStateUpdate: Uint8Array
}

const sendPeeringRequest = (
  sig: ActorContext,
  { peerNodeId, subnetId, lastLinkStateUpdate }: PeeringRequestParameters
) => {
  logger.debug(`sending peering request`, {
    to: peerNodeId,
  })

  sig.use(sendPeerMessage).tell("send", {
    destination: peerNodeId,
    message: {
      type: "peeringRequest",
      value: {
        nodeInfo: lastLinkStateUpdate,
        subnetId,
      },
    },
  })
}

interface HeartbeatParameters {
  peerNodeId: NodeId
  lastLinkStateUpdate: Uint8Array
}

const sendHeartbeat = (
  sig: ActorContext,
  { peerNodeId, lastLinkStateUpdate }: HeartbeatParameters
) => {
  logger.debug(`sending heartbeat`, {
    to: peerNodeId,
  })

  sig.use(sendPeerMessage).tell("send", {
    destination: peerNodeId,
    message: {
      type: "linkStateUpdate",
      value: {
        bytes: lastLinkStateUpdate,
      },
    },
  })
}
