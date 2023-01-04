import { createLogger } from "@dassie/lib-logger"
import { EffectContext, createTopic } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { incomingIlpPacketTopic } from "../ilp-connector/topics/incoming-ilp-packet"
import type { PeerMessage } from "./peer-schema"
import type { PerSubnetParameters } from "./run-per-subnet-effects"
import { NodeTableKey, nodeTableStore } from "./stores/node-table"
import { peerTableStore } from "./stores/peer-table"

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

    if (content.peeringRequest) {
      const { nodeInfo } = content.peeringRequest
      const { subnetId, nodeId, url } = nodeInfo.signed
      sig.use(peerTableStore).addPeer({
        subnetId,
        nodeId,
        state: { id: "peered" },
        url,
        lastSeen: Date.now(),
      })
    } else if (content.linkStateUpdate) {
      const { value: linkState, bytes: linkStateBytes } =
        content.linkStateUpdate
      const { nodeId, sequence, entries, nodePublicKey } = linkState.signed
      const nodes = sig.use(nodeTableStore).read()
      const peers = sig.use(peerTableStore).read()

      const neighbors = entries
        .filter((entry) => "neighbor" in entry)
        .map((entry) => entry.neighbor.nodeId)

      const nodeKey: NodeTableKey = `${subnetId}.${nodeId}`
      const node = nodes.get(nodeKey)
      const peer = peers.get(nodeKey)

      if (node) {
        if (sequence < node.sequence) {
          logger.debug("received a stale link state update", {
            subnet: subnetId,
            from: nodeId,
            sequence,
            neighbors: neighbors.join(","),
            previousSequence: node.sequence,
          })
          return
        }

        if (sequence === node.sequence) {
          if (peer) {
            logger.debug("received heartbeat from peer", {
              subnet: subnetId,
              from: nodeId,
              sequence,
            })
          } else {
            logger.debug("received another copy of a link state update", {
              subnet: subnetId,
              from: nodeId,
              sequence,
              counter: node.updateReceivedCounter + 1,
            })
          }

          sig.use(nodeTableStore).updateNode(nodeKey, {
            updateReceivedCounter: node.updateReceivedCounter + 1,
          })
          return
        }

        logger.debug("process new link state update", {
          subnet: subnetId,
          from: nodeId,
          sequence,
          neighbors: neighbors.join(","),
        })
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

      if (peer) {
        // If this peer has started sending us node updates, we can infer that we are successfully peered
        if (peer.state.id === "request-peering") {
          sig.use(peerTableStore).updatePeer(nodeKey, {
            state: { id: "peered" },
          })
        }

        sig.use(peerTableStore).updatePeer(nodeKey, {
          lastSeen: Date.now(),
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
