import { createLogger } from "@dassie/lib-logger"
import { EffectContext, createTopic } from "@dassie/lib-reactive"

import { incomingIlpPacketBuffer } from "../ilp-connector/topics/incoming-ilp-packet"
import type { PeerMessage } from "./peer-schema"
import { nodeTableStore } from "./stores/node-table"
import { peerTableStore } from "./stores/peer-table"

const logger = createLogger("das:node:incoming-peer-message-handler")

const MAX_LINK_STATE_UPDATE_RETRANSMIT_DELAY = 500

export interface IncomingPeerMessageEvent {
  message: PeerMessage
  authenticatedAs: string | undefined
  asUint8Array: Uint8Array
}

export const incomingPeerMessageTopic = () =>
  createTopic<IncomingPeerMessageEvent>()

export const handlePeerMessages = (sig: EffectContext) => {
  sig.on(incomingPeerMessageTopic, ({ message }) => {
    const content = message.content.value

    if (content.hello) {
      const { nodeInfo } = content.hello.signed
      const { nodeId, sequence, url } = nodeInfo.signed
      const peers = sig.use(peerTableStore).read()

      logger.debug("handle hello", {
        from: nodeId,
        sequence,
      })

      const peer = peers.get(nodeId)
      if (peer) {
        sig.use(peerTableStore).updatePeer(nodeId, {
          lastSeen: Date.now(),
        })
      } else {
        sig.use(peerTableStore).addPeer({
          nodeId,
          url,
          lastSeen: Date.now(),
        })
      }
    } else if (content.linkStateUpdate) {
      const { value: linkState, bytes: linkStateBytes } =
        content.linkStateUpdate
      const { nodeId, sequence, entries, nodeKey } = linkState.signed
      const nodes = sig.use(nodeTableStore).read()

      const neighbors = entries
        .filter((entry) => "neighbor" in entry)
        .map((entry) => entry.neighbor.nodeId)

      logger.debug("handle link state update", {
        from: nodeId,
        sequence,
        neighbors: neighbors.join(","),
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

          sig.use(nodeTableStore).updateNode(nodeId, {
            updateReceivedCounter: node.updateReceivedCounter + 1,
          })
          return
        }
        sig.use(nodeTableStore).updateNode(nodeId, {
          sequence,
          neighbors,
          lastLinkStateUpdate: linkStateBytes,
          updateReceivedCounter: 1,
          scheduledRetransmitTime:
            Date.now() +
            Math.ceil(Math.random() * MAX_LINK_STATE_UPDATE_RETRANSMIT_DELAY),
        })
      } else {
        sig.use(nodeTableStore).addNode({
          nodeId,
          nodeKey,
          sequence,
          updateReceivedCounter: 1,
          scheduledRetransmitTime:
            Date.now() +
            Math.ceil(Math.random() * MAX_LINK_STATE_UPDATE_RETRANSMIT_DELAY),
          neighbors,
          lastLinkStateUpdate: linkStateBytes,
        })
      }
    } else {
      logger.debug("handle interledger packet")

      sig.use(incomingIlpPacketBuffer).emit({
        source: content.interledgerPacket.signed.source,
        packet: content.interledgerPacket.signed.packet,
        requestId: content.interledgerPacket.signed.requestId,
      })
    }
  })
}
