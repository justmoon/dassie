import { createLogger } from "@dassie/lib-logger"
import { EffectContext, createTopic } from "@dassie/lib-reactive"

import type { PeerMessage } from "./peer-schema"
import { addNode, nodeTableStore, updateNode } from "./stores/node-table"
import { addPeer, peerTableStore, updatePeer } from "./stores/peer-table"

const logger = createLogger("das:node:incoming-peer-message-handler")

const MAX_LINK_STATE_UPDATE_RETRANSMIT_DELAY = 500

export interface IncomingPeerMessageEvent {
  message: PeerMessage
  asUint8Array: Uint8Array
}

export const incomingPeerMessageTopic = () =>
  createTopic<IncomingPeerMessageEvent>()

export const handlePeerMessages = (sig: EffectContext) => {
  sig.on(incomingPeerMessageTopic, ({ message }) => {
    if (message.hello) {
      const { nodeInfo } = message.hello.signed
      const { nodeId, sequence, url } = nodeInfo.signed
      const peers = sig.read(peerTableStore)

      logger.debug("handle hello", {
        from: nodeId,
        sequence,
      })

      const peer = peers.get(nodeId)
      if (peer) {
        sig.use(peerTableStore).update(
          updatePeer(nodeId, {
            lastSeen: Date.now(),
          })
        )
      } else {
        sig.use(peerTableStore).update(
          addPeer({
            nodeId,
            url,
            lastSeen: Date.now(),
          })
        )
      }
    } else {
      const { value: linkState, bytes: linkStateBytes } =
        message.linkStateUpdate
      const { nodeId, sequence, entries } = linkState.signed
      const nodes = sig.read(nodeTableStore)

      const neighbors = entries
        .filter((entry) => "neighbor" in entry)
        .map((entry) => entry.neighbor.nodeId)

      logger.debug("handle link state update", {
        from: nodeId,
        sequence,
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

          sig.use(nodeTableStore).update(
            updateNode(nodeId, {
              updateReceivedCounter: node.updateReceivedCounter + 1,
            })
          )
          return
        }
        sig.use(nodeTableStore).update(
          updateNode(nodeId, {
            sequence: sequence,
            updateReceivedCounter: 1,
            scheduledRetransmitTime:
              Date.now() +
              Math.ceil(Math.random() * MAX_LINK_STATE_UPDATE_RETRANSMIT_DELAY),
          })
        )
      } else {
        sig.use(nodeTableStore).update(
          addNode({
            nodeId,
            sequence,
            updateReceivedCounter: 1,
            scheduledRetransmitTime:
              Date.now() +
              Math.ceil(Math.random() * MAX_LINK_STATE_UPDATE_RETRANSMIT_DELAY),
            neighbors,
            lastLinkStateUpdate: linkStateBytes,
          })
        )
      }
    }
  })
}
