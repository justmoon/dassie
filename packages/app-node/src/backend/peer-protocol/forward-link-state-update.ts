import { createActor } from "@dassie/lib-reactive"
import { isFailure } from "@dassie/lib-type-utils"

import { peerProtocol as logger } from "../logger/instances"
import { SendPeerMessageActor } from "./actors/send-peer-message"
import { PeersSignal } from "./computed/peers"
import { peerMessageContent } from "./peer-schema"
import { NodeTableStore } from "./stores/node-table"

const COUNTER_THRESHOLD = 3
const MAX_RETRANSMIT_CHECK_INTERVAL = 200

export const ForwardLinkStateUpdateActor = () =>
  createActor((sig) => {
    const nodes = sig.readAndTrack(NodeTableStore)
    const peers = sig.readAndTrack(PeersSignal)

    for (const node of nodes.values()) {
      if (
        node.linkState &&
        node.linkState.updateReceivedCounter < COUNTER_THRESHOLD &&
        node.linkState.scheduledRetransmitTime < Date.now()
      ) {
        // Set scheduled retransmit time to be infinitely far in the future so we don't retransmit the same update again.
        sig.reactor.use(NodeTableStore).updateNode(node.nodeId, {
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

        if (isFailure(message)) {
          throw new Error("Failed to serialize link state update message", {
            cause: message,
          })
        }

        for (const peer of peers) {
          if (peer === node.nodeId) continue

          logger.debug("retransmit link state update", {
            from: node.nodeId,
            to: peer,
            sequence: node.linkState.sequence,
          })

          // Retransmit the link state update
          sig.reactor.use(SendPeerMessageActor).api.send.tell({
            destination: peer,
            message: {
              type: "linkStateUpdate",
              value: {
                bytes: node.linkState.lastUpdate,
              },
            },
            asUint8Array: message,
          })
        }
      }
    }

    sig.timeout(sig.forceRestart, MAX_RETRANSMIT_CHECK_INTERVAL)
  })
