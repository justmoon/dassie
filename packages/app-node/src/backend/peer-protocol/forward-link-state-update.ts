import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { sendPeerMessage } from "./actors/send-peer-message"
import { peersComputation } from "./computed/peers"
import { peerMessageContent } from "./peer-schema"
import { nodeTableStore, parseNodeKey } from "./stores/node-table"

const logger = createLogger("das:node:forward-link-state-update")

const COUNTER_THRESHOLD = 3
const MAX_RETRANSMIT_CHECK_INTERVAL = 200

export const forwardLinkStateUpdate = () =>
  createActor((sig) => {
    const nodes = sig.get(nodeTableStore)
    const peers = sig.get(peersComputation)

    for (const node of nodes.values()) {
      if (
        node.linkState.lastUpdate &&
        node.linkState.updateReceivedCounter < COUNTER_THRESHOLD &&
        node.linkState.scheduledRetransmitTime < Date.now()
      ) {
        // Set scheduled retransmit time to be infinitely far in the future so we don't retransmit the same update again.
        sig.use(nodeTableStore).updateNode(`${node.subnetId}.${node.nodeId}`, {
          linkState: {
            ...node.linkState,
            scheduledRetransmitTime: Number.POSITIVE_INFINITY,
          },
        })

        const message = peerMessageContent.serialize({
          type: "linkStateUpdate",
          value: {
            bytes: node.linkState.lastUpdate,
          },
        })

        if (!message.success) {
          throw new Error("Failed to serialize link state update message", {
            cause: message.error,
          })
        }

        for (const peer of peers) {
          const [peerSubnetId, peerNodeId] = parseNodeKey(peer)
          if (peerNodeId === node.nodeId && peerSubnetId === node.subnetId)
            continue

          logger.debug("retransmit link state update", {
            from: node.nodeId,
            to: peerNodeId,
            sequence: node.linkState.sequence,
          })

          // Retransmit the link state update
          sig.use(sendPeerMessage).tell("send", {
            subnet: peerSubnetId,
            destination: peerNodeId,
            message: {
              type: "linkStateUpdate",
              value: {
                bytes: node.linkState.lastUpdate,
              },
            },
            asUint8Array: message.value,
          })
        }
      }
    }

    sig.timeout(sig.wake.bind(sig), MAX_RETRANSMIT_CHECK_INTERVAL)
  })
