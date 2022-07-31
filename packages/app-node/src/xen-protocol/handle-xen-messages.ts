import { createLogger } from "@xen-ilp/lib-logger"
import { EffectContext, createTopic } from "@xen-ilp/lib-reactive"

import {
  addNode,
  nodeTableStore,
  updateNode,
} from "../peering/stores/node-table"
import {
  addPeer,
  peerTableStore,
  updatePeer,
} from "../peering/stores/peer-table"
import type { XenMessage } from "./xen-schema"

const logger = createLogger("xen:node:incoming-xen-message-handler")

const MAX_LINK_STATE_UPDATE_RETRANSMIT_DELAY = 500

export interface IncomingXenMessageEvent {
  message: XenMessage
  asUint8Array: Uint8Array
}

export const incomingXenMessageTopic = () =>
  createTopic<IncomingXenMessageEvent>()

export const handleXenMessages = (sig: EffectContext) => {
  sig.on(incomingXenMessageTopic, ({ message, asUint8Array }) => {
    if (message.hello) {
      const { nodeId, sequence, neighbors, url } = message.hello.signed
      const peers = sig.read(peerTableStore)

      logger.debug("handle hello", {
        from: nodeId,
        sequence,
        neighbors: () => neighbors.map((neighbor) => neighbor.nodeId).join(","),
      })

      const peer = peers.get(nodeId)
      if (peer) {
        if (sequence <= peer.sequence) {
          logger.debug("ignoring stale hello", {
            from: nodeId,
            sequence,
            previousSequence: peer.sequence,
          })
          return
        }

        sig.emit(
          peerTableStore,
          updatePeer(nodeId, {
            sequence: sequence,
            lastSeen: Date.now(),
          })
        )
      } else {
        sig.emit(
          peerTableStore,
          addPeer({
            nodeId,
            url,
            sequence: sequence,
            lastSeen: Date.now(),
          })
        )
      }
    } else {
      const { nodeId, sequence, neighbors } = message.linkStateUpdate.signed
      const nodes = sig.read(nodeTableStore)

      logger.debug("handle link state update", {
        from: nodeId,
        sequence,
        neighbors: () => neighbors.map((neighbor) => neighbor.nodeId).join(","),
      })

      const node = nodes.get(nodeId)
      if (node) {
        if (sequence < node.sequence) {
          logger.debug("received a stale link state update", {
            from: nodeId,
            sequence,
            previousSequence: node.sequence,
          })
          return
        }
        if (sequence === node.sequence) {
          logger.debug("received another copy of a link state update", {
            from: nodeId,
            sequence,
            counter: node.updateReceivedCounter + 1,
          })

          sig.emit(
            nodeTableStore,
            updateNode(nodeId, {
              updateReceivedCounter: node.updateReceivedCounter + 1,
            })
          )
          return
        }
        sig.emit(
          nodeTableStore,
          updateNode(nodeId, {
            sequence: sequence,
            updateReceivedCounter: 1,
            scheduledRetransmitTime:
              Date.now() +
              Math.ceil(Math.random() * MAX_LINK_STATE_UPDATE_RETRANSMIT_DELAY),
          })
        )
      } else {
        sig.emit(
          nodeTableStore,
          addNode({
            nodeId,
            sequence,
            updateReceivedCounter: 1,
            scheduledRetransmitTime:
              Date.now() +
              Math.ceil(Math.random() * MAX_LINK_STATE_UPDATE_RETRANSMIT_DELAY),
            neighbors: neighbors.map((neighbor) => neighbor.nodeId),
            lastLinkStateUpdate: asUint8Array,
          })
        )
      }
    }
  })
}
