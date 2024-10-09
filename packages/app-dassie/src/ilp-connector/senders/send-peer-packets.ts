import { assert } from "@dassie/lib-logger"
import { tell } from "@dassie/lib-type-utils"

import type { AccountPath } from "../../accounting/types/account-paths"
import type { DassieReactor } from "../../base/types/dassie-base"
import { connector as logger } from "../../logger/instances"
import { SendPeerMessage } from "../../peer-protocol/functions/send-peer-message"
import type { NodeId } from "../../peer-protocol/types/node-id"
import type { PacketSender } from "../functions/send-packet"

export interface PeerEndpointInfo {
  readonly type: "peer"
  readonly nodeId: NodeId
  readonly accountPath: AccountPath
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

      assert(
        logger,
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
