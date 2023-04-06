import { createLogger } from "@dassie/lib-logger"
import { EffectContext, createActor } from "@dassie/lib-reactive"

import { nodeIdSignal } from "../ilp-connector/computed/node-id"
import { sendPeerMessage } from "./actions/send-peer-message"
import { peersComputation } from "./computed/peers"
import { requestedPeersComputation } from "./computed/requested-peers"
import { nodeTableStore, parseNodeKey } from "./stores/node-table"

const logger = createLogger("das:node:peer-greeter")

const MAX_HEARTBEAT_INTERVAL = 20_000

export const sendHeartbeats = () =>
  createActor((sig) => {
    const ownNodeId = sig.use(nodeIdSignal).read()

    // Get the current peers and re-run the actor if they change
    const peers = sig.get(peersComputation)
    const requestedPeers = sig.get(requestedPeersComputation)

    const linkStateUpdateCache = new Map<string, Uint8Array>()
    const getLinkStateUpdate = (subnetId: string) => {
      if (!linkStateUpdateCache.has(subnetId)) {
        const ownNodeTableEntry = sig.get(nodeTableStore, (nodeTable) =>
          nodeTable.get(`${subnetId}.${ownNodeId}`)
        )

        if (!ownNodeTableEntry?.linkState.lastUpdate) {
          return undefined
        }

        linkStateUpdateCache.set(
          subnetId,
          ownNodeTableEntry.linkState.lastUpdate
        )
      }

      return linkStateUpdateCache.get(subnetId)
    }

    // Send peer requests
    for (const peer of requestedPeers) {
      const [peerSubnetId, peerNodeId] = parseNodeKey(peer)

      const linkStateUpdate = getLinkStateUpdate(peerSubnetId)

      if (!linkStateUpdate) {
        continue
      }

      sendPeeringRequest(sig, {
        peerSubnetId,
        peerNodeId,
        lastLinkStateUpdate: linkStateUpdate,
      })
    }

    // Send heartbeats to existing peers
    for (const peer of peers) {
      const [peerSubnetId, peerNodeId] = parseNodeKey(peer)

      const linkStateUpdate = getLinkStateUpdate(peerSubnetId)

      if (!linkStateUpdate) {
        continue
      }

      sendHeartbeat(sig, {
        peerSubnetId,
        peerNodeId,
        lastLinkStateUpdate: linkStateUpdate,
      })
    }

    sig.timeout(sig.wake, Math.random() * MAX_HEARTBEAT_INTERVAL)
  })

interface HeartbeatParameters {
  peerSubnetId: string
  peerNodeId: string
  lastLinkStateUpdate: Uint8Array
}

const sendPeeringRequest = (
  sig: EffectContext,
  { peerSubnetId, peerNodeId, lastLinkStateUpdate }: HeartbeatParameters
) => {
  logger.debug(`sending peering request`, {
    subnet: peerSubnetId,
    to: peerNodeId,
  })

  sig.use(sendPeerMessage).tell({
    subnet: peerSubnetId,
    destination: peerNodeId,
    message: {
      type: "peeringRequest",
      value: {
        nodeInfo: lastLinkStateUpdate,
      },
    },
  })
}

const sendHeartbeat = (
  sig: EffectContext,
  { peerSubnetId, peerNodeId, lastLinkStateUpdate }: HeartbeatParameters
) => {
  logger.debug(`sending heartbeat`, {
    subnet: peerSubnetId,
    to: peerNodeId,
  })

  sig.use(sendPeerMessage).tell({
    subnet: peerSubnetId,
    destination: peerNodeId,
    message: {
      type: "linkStateUpdate",
      value: {
        bytes: lastLinkStateUpdate,
      },
    },
  })
}
