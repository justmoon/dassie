import { createLogger } from "@xen-ilp/lib-logger"
import type { EffectContext } from "@xen-ilp/lib-reactive"

import { configStore } from "../config"
import { signerValue } from "../signer/signer"
import { compareSetOfKeys } from "../utils/compare-set-of-keys"
import { outgoingXenMessageBufferTopic } from "../xen-protocol/send-xen-messages"
import { xenMessage, xenSignedHello } from "../xen-protocol/xen-schema"
import { PeerEntry, peerTableStore } from "./stores/peer-table"

const logger = createLogger("xen:node:peer-greeter")

const MAX_GREETING_INTERVAL = 20_000

export const greetPeers = (sig: EffectContext) => {
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

  const sendHello = (peer: PeerEntry) => {
    const sequence = BigInt(Date.now())

    logger.debug(`sending hello`, { to: peer.nodeId, sequence })

    const signedHello = xenSignedHello.serialize({
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

    const signature = signer.signWithXenKey(signedHello.value)

    const messageSerializeResult = xenMessage.serialize({
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

    sig.emit(outgoingXenMessageBufferTopic, {
      destination: peer.nodeId,
      message: messageSerializeResult.value,
    })
  }

  let timer: NodeJS.Timer | undefined = undefined
  const tick = () => {
    for (const peer of peers.values()) {
      sendHello(peer)
    }

    // Schedule the next tick. Intervals are uniformly distributed between 0 and the maximum greeting interval.
    timer = setTimeout(tick, Math.random() * MAX_GREETING_INTERVAL)
  }

  tick()

  sig.onCleanup(() => {
    if (timer) clearTimeout(timer)
  })
}
