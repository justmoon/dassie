import { createLogger } from "@dassie/lib-logger"
import type { EffectContext } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { signerService } from "../crypto/signer"
import { compareSetOfKeys } from "../utils/compare-sets"
import { sendPeerMessage } from "./send-peer-messages"
import { nodeTableStore } from "./stores/node-table"
import { PeerEntry, peerTableStore } from "./stores/peer-table"

const logger = createLogger("das:node:peer-greeter")

const MAX_HEARTBEAT_INTERVAL = 20_000

export const sendHeartbeats = (sig: EffectContext) => {
  const signer = sig.get(signerService)

  if (!signer) return

  // Get the current peers and re-run the effect iff the IDs of the peers change.
  const peers = sig.get(
    peerTableStore,
    (peerTable) => peerTable,
    compareSetOfKeys
  )

  const ownNodeId = sig.use(configSignal).read().nodeId

  for (const peer of peers.values()) {
    const ownNodeTableEntry = sig.get(nodeTableStore, (nodeTable) =>
      nodeTable.get(`${peer.subnetId}.${ownNodeId}`)
    )

    if (!ownNodeTableEntry?.lastLinkStateUpdate) {
      return
    }

    switch (peer.state.id) {
      case "request-peering": {
        sendPeeringRequest(sig, {
          peer,
          lastLinkStateUpdate: ownNodeTableEntry.lastLinkStateUpdate,
        })
        break
      }
      case "peered": {
        sendHeartbeat(sig, {
          peer,
          lastLinkStateUpdate: ownNodeTableEntry.lastLinkStateUpdate,
        })
        break
      }
    }
  }

  sig.timeout(sig.wake, Math.random() * MAX_HEARTBEAT_INTERVAL)
}

interface HeartbeatParameters {
  peer: PeerEntry
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

  sig
    .use(sendPeerMessage)({
      subnet: peer.subnetId,
      destination: peer.nodeId,
      message: {
        peeringRequest: {
          nodeInfo: lastLinkStateUpdate,
        },
      },
    })
    .catch((error: unknown) => {
      logger.warn(`failed to send peering request`, {
        subnet: peer.subnetId,
        to: peer.nodeId,
        error,
      })
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

  sig
    .use(sendPeerMessage)({
      subnet: peer.subnetId,
      destination: peer.nodeId,
      message: {
        linkStateUpdate: {
          bytes: lastLinkStateUpdate,
        },
      },
    })
    .catch((error: unknown) => {
      logger.warn(`failed to send heartbeat`, {
        subnet: peer.subnetId,
        to: peer.nodeId,
        error,
      })
    })
}
