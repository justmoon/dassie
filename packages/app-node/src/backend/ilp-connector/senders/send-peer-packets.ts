import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { nodeTableStore } from "../.."
import { sendPeerMessage } from "../../peer-protocol/actors/send-peer-message"
import { NodeId } from "../../peer-protocol/types/node-id"
import { PacketSender } from "../functions/send-packet"

export interface PeerDestinationInfo {
  type: "peer"
  firstHopOptions: NodeId[]
  distance: number
}

const logger = createLogger("das:node:send-interledger-packets")

export const sendPeerPackets = () =>
  createActor((sig) => {
    const nodeTable = sig.use(nodeTableStore)

    return {
      sendPrepare: ({
        firstHopOptions,
        serializedPacket,
        outgoingRequestId: requestId,
      }) => {
        const nextHop = firstHopOptions[0]!

        const peerState = nodeTable.read().get(nextHop)?.peerState

        if (peerState?.id !== "peered") {
          throw new Error(`Next hop node is not actually peered ${nextHop}`)
        }

        logger.debug("sending ilp packet", { nextHop })

        sig.use(sendPeerMessage).tell("send", {
          destination: nextHop,
          message: {
            type: "interledgerPacket",
            value: {
              signed: {
                requestId,
                packet: serializedPacket,
              },
            },
          },
        })
      },
      sendResult: ({
        firstHopOptions,
        serializedPacket,
        prepare: { incomingRequestId: requestId },
      }) => {
        const nextHop = firstHopOptions[0]!

        const peerState = sig.use(nodeTableStore).read().get(nextHop)?.peerState

        if (peerState?.id !== "peered") {
          throw new Error(`Next hop node is not actually peered ${nextHop}`)
        }

        logger.debug("sending ilp packet", { nextHop })

        sig.use(sendPeerMessage).tell("send", {
          destination: nextHop,
          message: {
            type: "interledgerPacket",
            value: {
              signed: {
                requestId: requestId,
                packet: serializedPacket,
              },
            },
          },
        })
      },
    }
  }) satisfies PacketSender<"peer">
