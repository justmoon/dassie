import { createLogger } from "@xen-ilp/lib-logger"
import type { EffectContext } from "@xen-ilp/lib-reactive"

import { peerTableStore, updatePeer } from "../peering/stores/peer-table"
import { XenMessageType } from "./codecs/xen-message"
import { incomingXenMessageTopic } from "./topics/xen-protocol"

const logger = createLogger("xen:node:incoming-xen-message-handler")

export const incomingXenMessageHandler = (sig: EffectContext) => {
  const peers = sig.get(peerTableStore)

  sig.on(incomingXenMessageTopic, (message) => {
    switch (message.method) {
      case XenMessageType.Hello: {
        const { nodeId, sequence, neighbors } = message.signed

        logger.debug("handle hello", {
          from: nodeId,
          sequence,
          neighbors: () =>
            neighbors.map((neighbor) => neighbor.nodeId).join(","),
        })

        const peer = peers[nodeId]
        if (peer) {
          if (sequence < peer.theirSequence) {
            logger.debug("ignoring stale hello", { from: nodeId })
            return
          }

          sig.reactor.emit(
            peerTableStore,
            updatePeer(nodeId, {
              theirSequence: sequence,
              lastSeen: Date.now(),
            })
          )
        }

        break
      }
      default:
        logger.debug("ignoring unknown message", { method: message.method })
    }
  })
}
