import { createLogger } from "@xen-ilp/lib-logger"
import type { EffectContext } from "@xen-ilp/lib-reactive"

import { configStore } from "../config"
import { signerValue } from "../crypto/signer"
import { peerMessage, peerSignedHello } from "../peer-protocol/peer-schema"
import { outgoingPeerMessageBufferTopic } from "../peer-protocol/send-peer-messages"
import { compareSetOfKeys } from "../utils/compare-sets"
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

  const { nodeId, port } = sig.get(configStore, ({ nodeId, port }) => ({
    nodeId,
    port,
  }))

  const signer = sig.get(signerValue)

  for (const peer of peers.values()) {
    const sequence = BigInt(Date.now())

    logger.debug(`sending hello`, { to: peer.nodeId, sequence })

    const signedHello = peerSignedHello.serialize({
      nodeId,
      sequence,
      url: `https://${nodeId}.localhost:${port}`,
      neighbors: [...peers.values()].map((peer) => ({
        nodeId: peer.nodeId,
        proof: new Uint8Array(32),
      })),
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