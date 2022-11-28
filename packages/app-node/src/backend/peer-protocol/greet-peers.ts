import { createLogger } from "@dassie/lib-logger"
import type { EffectContext } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { signerService } from "../crypto/signer"
import { peerHello, peerMessageContent } from "../peer-protocol/peer-schema"
import { outgoingPeerMessageBufferTopic } from "../peer-protocol/send-peer-messages"
import { compareSetOfKeys } from "../utils/compare-sets"
import { nodeTableStore } from "./stores/node-table"
import { peerTableStore } from "./stores/peer-table"

const logger = createLogger("das:node:peer-greeter")

const MAX_GREETING_INTERVAL = 20_000

export const greetPeers = async (sig: EffectContext) => {
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
    const sequence = BigInt(Date.now())

    const ownNodeTableEntry = sig.get(nodeTableStore, (nodeTable) =>
      nodeTable.get(`${peer.subnetId}.${ownNodeId}`)
    )

    if (!ownNodeTableEntry) {
      return
    }

    logger.debug(`sending hello`, {
      subnet: peer.subnetId,
      to: peer.nodeId,
      sequence,
    })

    const signedHello = peerHello.serialize({
      nodeInfo: ownNodeTableEntry.lastLinkStateUpdate,
    })

    if (!signedHello.success) {
      logger.warn("Failed to serialize hello message", {
        error: signedHello.error,
      })
      return
    }

    const signature = await signer.signWithDassieKey(signedHello.value)

    const messageSerializeResult = peerMessageContent.serialize({
      hello: {
        signed: signedHello.value,
        signature,
      },
    })

    if (!messageSerializeResult.success) {
      logger.warn("Failed to serialize hello message", {
        error: messageSerializeResult.error,
      })
      return
    }

    sig.use(outgoingPeerMessageBufferTopic).emit({
      subnet: peer.subnetId,
      destination: peer.nodeId,
      message: messageSerializeResult.value,
    })
  }

  sig.timeout(sig.wake, Math.random() * MAX_GREETING_INTERVAL)
}
