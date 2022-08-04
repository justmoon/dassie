import { createLogger } from "@xen-ilp/lib-logger"
import type { EffectContext } from "@xen-ilp/lib-reactive"

import { configStore } from "../config"
import { signerValue } from "../crypto/signer"
import { peerHello, peerMessage } from "../peer-protocol/peer-schema"
import { outgoingPeerMessageBufferTopic } from "../peer-protocol/send-peer-messages"
import { compareSetOfKeys } from "../utils/compare-sets"
import { nodeTableStore } from "./stores/node-table"
import { peerTableStore } from "./stores/peer-table"

const logger = createLogger("xen:node:peer-greeter")

const MAX_GREETING_INTERVAL = 20_000

export const greetPeers = async (sig: EffectContext) => {
  // Get the current peers and re-run the effect iff the IDs of the peers change.
  const peers = sig.get(
    peerTableStore,
    (peerTable) => peerTable,
    compareSetOfKeys
  )

  const ownNodeTableEntry = sig.get(nodeTableStore, (nodeTable) =>
    nodeTable.get(sig.read(configStore).nodeId)
  )

  if (!ownNodeTableEntry) {
    return
  }

  const signer = sig.get(signerValue)

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

    const signature = await signer.signWithXenKey(signedHello.value)

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

    sig.emit(outgoingPeerMessageBufferTopic, {
      destination: peer.nodeId,
      message: messageSerializeResult.value,
    })
  }

  sig.timeout(sig.wake, Math.random() * MAX_GREETING_INTERVAL)
}
