import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { sendPeerMessage } from "../../peer-protocol/actors/send-peer-message"
import { NodeId } from "../../peer-protocol/types/node-id"
import { CommonEndpointInfo, PacketSender } from "../functions/send-packet"

export interface PeerEndpointInfo extends CommonEndpointInfo {
  readonly type: "peer"
  readonly nodeId: NodeId
}

const logger = createLogger("das:node:send-interledger-packets")

export const sendPeerPackets = () =>
  createActor((sig) => {
    return {
      sendPrepare: ({
        nodeId,
        serializedPacket,
        outgoingRequestId: requestId,
      }) => {
        logger.debug("sending ilp packet", { nextHop: nodeId })

        sig.use(sendPeerMessage).tell("send", {
          destination: nodeId,
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
        nodeId,
        serializedPacket,
        prepare: { incomingRequestId: requestId },
      }) => {
        logger.debug("sending ilp packet", { nextHop: nodeId })

        sig.use(sendPeerMessage).tell("send", {
          destination: nodeId,
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
    }
  }) satisfies PacketSender<"peer">
