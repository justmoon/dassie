import { createLogger } from "@dassie/lib-logger"
import { EffectContext, createTopic } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { incomingIlpPacketTopic } from "../ilp-connector/topics/incoming-ilp-packet"
import type { PeerMessage } from "./peer-schema"
import type { PerSubnetParameters } from "./run-per-subnet-effects"
import { NodeTableKey, nodeTableStore } from "./stores/node-table"
import { PeerTableKey, peerTableStore } from "./stores/peer-table"

const logger = createLogger("das:node:incoming-peer-message-handler")

const MAX_LINK_STATE_UPDATE_RETRANSMIT_DELAY = 500

export interface IncomingPeerMessageEvent {
  message: PeerMessage
  authenticated: boolean
  asUint8Array: Uint8Array
}

export const incomingPeerMessageTopic = () =>
  createTopic<IncomingPeerMessageEvent>()

export const handlePeerMessages = (
  sig: EffectContext,
  parameters: PerSubnetParameters
) => {
  sig.onAsync(incomingPeerMessageTopic, async ({ message, authenticated }) => {
    const { subnetId, sender } = message

    if (subnetId !== parameters.subnetId) {
      return
    }

    const content = message.content.value

    if (content.hello) {
      const { nodeInfo } = content.hello.signed
      const { nodeId, sequence, url } = nodeInfo.signed
      const peers = sig.use(peerTableStore).read()

      logger.debug("handle hello", {
        subnet: subnetId,
        from: nodeId,
        sequence,
      })

      const peerKey: PeerTableKey = `${subnetId}.${nodeId}`
      const peer = peers.get(peerKey)
      if (peer) {
        sig.use(peerTableStore).updatePeer(peerKey, {
          lastSeen: Date.now(),
        })
      } else {
        sig.use(peerTableStore).addPeer({
          subnetId,
          nodeId,
          url,
          lastSeen: Date.now(),
        })
      }
    } else if (content.linkStateUpdate) {
      const { value: linkState, bytes: linkStateBytes } =
        content.linkStateUpdate
      const { nodeId, sequence, entries, nodePublicKey } = linkState.signed
      const nodes = sig.use(nodeTableStore).read()

      const neighbors = entries
        .filter((entry) => "neighbor" in entry)
        .map((entry) => entry.neighbor.nodeId)

      logger.debug("handle link state update", {
        subnet: subnetId,
        from: nodeId,
        sequence,
        neighbors: neighbors.join(","),
      })

      const nodeKey: NodeTableKey = `${subnetId}.${nodeId}`
      const node = nodes.get(nodeKey)
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

          sig.use(nodeTableStore).updateNode(nodeKey, {
            updateReceivedCounter: node.updateReceivedCounter + 1,
          })
          return
        }
        sig.use(nodeTableStore).updateNode(nodeKey, {
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
          subnetId,
          nodeId,
          nodePublicKey,
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
      if (!authenticated) {
        logger.warn("received unauthenticated interledger packet, discarding")
        return
      }

      logger.debug("handle interledger packet", {
        subnet: subnetId,
        from: sender,
      })

      const incomingIlpPacketTopicValue = sig.use(incomingIlpPacketTopic)

      const { ilpAllocationScheme } = sig.use(configSignal).read()
      const incomingPacketEvent = incomingIlpPacketTopicValue.prepareEvent({
        source: `${ilpAllocationScheme}.das.${subnetId}.${sender}`,
        packet: content.interledgerPacket.signed.packet,
        requestId: content.interledgerPacket.signed.requestId,
      })

      await parameters.subnetModule.processIncomingPacket({
        packet: incomingPacketEvent.packet,
      })

      incomingIlpPacketTopicValue.emit(incomingPacketEvent)
    }
  })
}
