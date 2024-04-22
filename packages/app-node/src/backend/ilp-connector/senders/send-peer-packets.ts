import { tell } from "@dassie/lib-type-utils"

import { DassieReactor } from "../../base/types/dassie-base"
import { connector as logger } from "../../logger/instances"
import { SendPeerMessage } from "../../peer-protocol/functions/send-peer-message"
import { NodeId } from "../../peer-protocol/types/node-id"
import { CommonEndpointInfo, PacketSender } from "../functions/send-packet"

export interface PeerEndpointInfo extends CommonEndpointInfo {
  readonly type: "peer"
  readonly nodeId: NodeId
}

export const SendPeerPackets = (
  reactor: DassieReactor,
): PacketSender<"peer"> => {
  const sendPeerMessage = reactor.use(SendPeerMessage)

  return {
    sendPrepare: ({
      destinationEndpointInfo: { nodeId },
      serializedPacket,
      outgoingRequestId: requestId,
    }) => {
      logger.debug?.("sending ilp packet", { nextHop: nodeId })
      tell(() =>
        sendPeerMessage({
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
        }),
      )
    },
    sendResult: ({
      destinationEndpointInfo: { nodeId },
      serializedPacket,
      prepare: { incomingRequestId: requestId },
    }) => {
      logger.debug?.("sending ilp packet", { nextHop: nodeId })

      logger.assert(
        typeof requestId === "number",
        "expected requestId to be a number for peer packets",
      )

      tell(() =>
        sendPeerMessage({
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
        }),
      )
    },
  }
}
