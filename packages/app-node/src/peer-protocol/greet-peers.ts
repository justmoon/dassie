import { createLogger } from "@dassie/lib-logger"
import type { EffectContext } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { signerService } from "../crypto/signer"
import { peerHello, peerMessage } from "../peer-protocol/peer-schema"
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

  const ownNodeTableEntry = sig.get(nodeTableStore, (nodeTable) =>
    nodeTable.get(sig.use(configSignal).read().nodeId)
  )

  if (!ownNodeTableEntry) {
    return
  }

  for (const peer of peers.values()) {
    const sequence = BigInt(Date.now())

    logger.debug(`sending hello`, { to: peer.nodeId, sequence })

    const signedHello = peerHello.serialize({
      nodeInfo: ownNodeTableEntry.lastLinkStateUpdate,
    })

    if (!signedHello.success) {
      logger.warn("Failed to serialize hello message", {
        error: signedHello.failure,
      })
      return
    }

    const signature = await signer.signWithDassieKey(signedHello.value)

    const messageSerializeResult = peerMessage.serialize({
      hello: {
        signed: signedHello.value,
        signature,
      },
    })

    if (!messageSerializeResult.success) {
      logger.warn("Failed to serialize hello message", {
        error: messageSerializeResult.failure,
      })
      return
    }

    sig.use(outgoingPeerMessageBufferTopic).emit({
      destination: peer.nodeId,
      message: messageSerializeResult.value,
    })
  }

  sig.timeout(sig.wake, Math.random() * MAX_GREETING_INTERVAL)
}
