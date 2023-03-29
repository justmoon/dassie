import { createLogger } from "@dassie/lib-logger"
import { EffectContext, createActor } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { signerService } from "../crypto/signer"
import { compareSets } from "../utils/compare-sets"
import { sendPeerMessage } from "./actions/send-peer-message"
import { peersComputation } from "./computed/peers"
import {
  NodeTableEntry,
  nodeTableStore,
  parseNodeKey,
} from "./stores/node-table"

const logger = createLogger("das:node:peer-greeter")

const MAX_HEARTBEAT_INTERVAL = 20_000

export const sendHeartbeats = () =>
  createActor((sig) => {
    const signer = sig.get(signerService)

    if (!signer) return

    // Get the current peers and re-run the effect iff the IDs of the peers change.
    const peers = sig.get(
      peersComputation,
      (peerTable) => peerTable,
      compareSets
    )

    const ownNodeId = sig.use(configSignal).read().nodeId

    for (const peer of peers) {
      const [peerSubnetId] = parseNodeKey(peer)
      const ownNodeTableEntry = sig.get(nodeTableStore, (nodeTable) =>
        nodeTable.get(`${peerSubnetId}.${ownNodeId}`)
      )

      if (!ownNodeTableEntry?.lastLinkStateUpdate) {
        return
      }

      const peerEntry = sig.use(nodeTableStore).read().get(peer)

      switch (peerEntry?.peerState.id) {
        case "request-peering": {
          sendPeeringRequest(sig, {
            peer: peerEntry,
            lastLinkStateUpdate: ownNodeTableEntry.lastLinkStateUpdate,
          })
          break
        }
        case "peered": {
          sendHeartbeat(sig, {
            peer: peerEntry,
            lastLinkStateUpdate: ownNodeTableEntry.lastLinkStateUpdate,
          })
          break
        }
      }
    }

    sig.timeout(sig.wake, Math.random() * MAX_HEARTBEAT_INTERVAL)
  })

interface HeartbeatParameters {
  peer: NodeTableEntry
  lastLinkStateUpdate: Uint8Array
}

const sendPeeringRequest = (
  sig: EffectContext,
  { peer, lastLinkStateUpdate }: HeartbeatParameters
) => {
  logger.debug(`sending peering request`, {
    subnet: peer.subnetId,
    to: peer.nodeId,
  })

  sig.use(sendPeerMessage).tell({
    subnet: peer.subnetId,
    destination: peer.nodeId,
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
  { peer, lastLinkStateUpdate }: HeartbeatParameters
) => {
  logger.debug(`sending heartbeat`, {
    subnet: peer.subnetId,
    to: peer.nodeId,
  })

  sig.use(sendPeerMessage).tell({
    subnet: peer.subnetId,
    destination: peer.nodeId,
    message: {
      type: "linkStateUpdate",
      value: {
        bytes: lastLinkStateUpdate,
      },
    },
  })
}
