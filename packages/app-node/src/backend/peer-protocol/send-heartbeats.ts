import assert from "node:assert"

import { ActorContext, createActor } from "@dassie/lib-reactive"

import { NodeIdSignal } from "../ilp-connector/computed/node-id"
import { peerProtocol as logger } from "../logger/instances"
import { SendPeerMessageActor } from "./actors/send-peer-message"
import { PeersSignal } from "./computed/peers"
import { RequestedPeersSignal } from "./computed/requested-peers"
import { NodeTableStore } from "./stores/node-table"
import { NodeId } from "./types/node-id"
import { SettlementSchemeId } from "./types/settlement-scheme-id"

const MAX_HEARTBEAT_INTERVAL = 20_000

export const SendHeartbeatsActor = () =>
  createActor((sig) => {
    const ownNodeId = sig.use(NodeIdSignal).read()

    // Get the current peers and re-run the actor if they change
    const peers = sig.get(PeersSignal)
    const requestedPeers = sig.get(RequestedPeersSignal)

    const linkStateUpdate = sig.get(NodeTableStore, (nodeTable) =>
      nodeTable.get(ownNodeId),
    )?.linkState.lastUpdate

    if (!linkStateUpdate) {
      return
    }

    // Send peer requests
    for (const peer of requestedPeers) {
      const peerState = sig.use(NodeTableStore).read().get(peer)?.peerState

      assert(peerState?.id === "request-peering")

      sendPeeringRequest(sig, {
        peerNodeId: peer,
        settlementSchemeId: peerState.settlementSchemeId,
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
  settlementSchemeId: SettlementSchemeId
  lastLinkStateUpdate: Uint8Array
}

const sendPeeringRequest = (
  sig: ActorContext,
  {
    peerNodeId,
    settlementSchemeId,
    lastLinkStateUpdate,
  }: PeeringRequestParameters,
) => {
  logger.debug(`sending peering request`, {
    to: peerNodeId,
  })

  sig.use(SendPeerMessageActor).tell("send", {
    destination: peerNodeId,
    message: {
      type: "peeringRequest",
      value: {
        nodeInfo: lastLinkStateUpdate,
        settlementSchemeId,
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
  { peerNodeId, lastLinkStateUpdate }: HeartbeatParameters,
) => {
  logger.debug(`sending heartbeat`, {
    to: peerNodeId,
  })

  sig.use(SendPeerMessageActor).tell("send", {
    destination: peerNodeId,
    message: {
      type: "linkStateUpdate",
      value: {
        bytes: lastLinkStateUpdate,
      },
    },
  })
}
