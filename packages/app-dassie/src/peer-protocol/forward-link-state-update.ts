import { createActor } from "@dassie/lib-reactive"
import { isFailure, tell } from "@dassie/lib-type-utils"

import type {
  DassieActorContext,
  DassieReactor,
} from "../base/types/dassie-base"
import { peerProtocol as logger } from "../logger/instances"
import { PeersSignal } from "./computed/peers"
import { SendPeerMessage } from "./functions/send-peer-message"
import { peerMessageContent } from "./peer-schema"
import { NodeTableStore } from "./stores/node-table"

const COUNTER_THRESHOLD = 3
const MAX_RETRANSMIT_CHECK_INTERVAL = 200

export const ForwardLinkStateUpdateActor = (reactor: DassieReactor) => {
  const nodeTableStore = reactor.use(NodeTableStore)
  const peersSignal = reactor.use(PeersSignal)
  const sendPeerMessage = reactor.use(SendPeerMessage)

  return createActor((sig: DassieActorContext) => {
    function forwardLinkStateUpdates() {
      const nodes = nodeTableStore.read()
      const peers = peersSignal.read()

      for (const node of nodes.values()) {
        if (
          node.linkState &&
          node.linkState.updateReceivedCounter < COUNTER_THRESHOLD &&
          node.linkState.scheduledRetransmitTime < Date.now()
        ) {
          // Set scheduled retransmit time to be infinitely far in the future so we don't retransmit the same update again.
          sig.reactor.use(NodeTableStore).act.updateNode(node.nodeId, {
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

            logger.debug?.("retransmit link state update", {
              from: node.nodeId,
              to: peer,
              sequence: node.linkState.sequence,
            })

            // Retransmit the link state update
            const linkStateUpdate = node.linkState.lastUpdate
            tell(() =>
              sendPeerMessage({
                destination: peer,
                message: {
                  type: "linkStateUpdate",
                  value: {
                    bytes: linkStateUpdate,
                  },
                },
                asUint8Array: message,
              }),
            )
          }
        }
      }
    }

    sig
      .task({
        handler: forwardLinkStateUpdates,
        interval: MAX_RETRANSMIT_CHECK_INTERVAL,
      })
      .schedule()
  })
}
