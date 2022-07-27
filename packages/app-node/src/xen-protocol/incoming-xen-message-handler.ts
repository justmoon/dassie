import { createLogger } from "@xen-ilp/lib-logger"
import type { EffectContext } from "@xen-ilp/lib-reactive"

import {
  addPeer,
  peerTableStore,
  updatePeer,
} from "../peering/stores/peer-table"
import { incomingXenMessageTopic } from "./topics/xen-protocol"

const logger = createLogger("xen:node:incoming-xen-message-handler")

export const incomingXenMessageHandler = (sig: EffectContext) => {
  const peers = sig.get(peerTableStore)

  sig.on(incomingXenMessageTopic, (envelope) => {
    const { nodeId, message } = envelope
    const { sequence, neighbors, url } = message.hello

    logger.debug("handle hello", {
      from: nodeId,
      sequence,
      neighbors: () => neighbors.map((neighbor) => neighbor.nodeId).join(","),
    })

    const peer = peers[nodeId]
    if (peer) {
      if (sequence <= peer.theirSequence) {
        logger.debug("ignoring stale hello", {
          from: nodeId,
          sequence,
          previousSequence: peer.theirSequence,
        })
        return
      }

      sig.emit(
        peerTableStore,
        updatePeer(nodeId, {
          theirSequence: sequence,
          lastSeen: Date.now(),
        })
      )
    } else {
      sig.emit(
        peerTableStore,
        addPeer({
          nodeId,
          url,
          theirSequence: sequence,
          lastSeen: Date.now(),
        })
      )
    }
  })
}
