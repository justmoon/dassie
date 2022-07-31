import { createLogger } from "@xen-ilp/lib-logger"
import type { EffectContext } from "@xen-ilp/lib-reactive"

import { configStore } from "../config"
import {
  peerMessage,
  peerSignedLinkStateUpdate,
} from "../peer-protocol/peer-schema"
import { outgoingPeerMessageBufferTopic } from "../peer-protocol/send-peer-messages"
import { signerValue } from "../signer/signer"
import { compareSetOfKeys } from "../utils/compare-set-of-keys"
import { PeerEntry, peerTableStore } from "./stores/peer-table"

const logger = createLogger("xen:node:publish-link-state-update")

export const publishLinkStateUpdate = (sig: EffectContext) => {
  // Get the current peers and re-run the effect iff the IDs of the peers change.
  const peers = sig.get(
    peerTableStore,
    (peerTable) => peerTable,
    compareSetOfKeys
  )

  const { nodeId } = sig.get(configStore, ({ nodeId }) => ({
    nodeId,
  }))

  const signer = sig.get(signerValue)

  const sendLinkStateUpdate = (peer: PeerEntry) => {
    const sequence = BigInt(Date.now())

    logger.debug(`sending link state update`, { to: peer.nodeId, sequence })

    const signedLinkStateUpdate = peerSignedLinkStateUpdate.serialize({
      nodeId,
      sequence,
      neighbors: [...peers.values()].map((peer) => ({
        nodeId: peer.nodeId,
      })),
    })

    if (!signedLinkStateUpdate.success) {
      logger.warn("Failed to serialize link state update signed portion", {
        error: signedLinkStateUpdate.failure,
      })
      return
    }

    const signature = signer.signWithXenKey(signedLinkStateUpdate.value)
    const message = peerMessage.serialize({
      linkStateUpdate: {
        signed: signedLinkStateUpdate.value,
        signature,
      },
    })

    if (!message.success) {
      logger.warn("Failed to serialize link state update message", {
        error: message.failure,
      })
      return
    }

    sig.emit(outgoingPeerMessageBufferTopic, {
      destination: peer.nodeId,
      message: message.value,
    })
  }

  for (const peer of peers.values()) {
    sendLinkStateUpdate(peer)
  }
}
