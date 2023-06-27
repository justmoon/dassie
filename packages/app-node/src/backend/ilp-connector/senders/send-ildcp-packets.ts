import { createActor } from "@dassie/lib-reactive"

import { handleIldcpRequests } from "../../ildcp-server/handle-ildcp-requests"
import { CommonEndpointInfo, PacketSender } from "../functions/send-packet"

export interface IldcpEndpointInfo extends CommonEndpointInfo {
  type: "ildcp"
}

export const sendIldcpPackets = () =>
  createActor((sig) => {
    const ildcpHandler = sig.use(handleIldcpRequests)

    return {
      sendPrepare: ({ sourceEndpointInfo, outgoingRequestId: requestId }) => {
        ildcpHandler.tell("handle", {
          sourceIlpAddress: sourceEndpointInfo.ilpAddress,
          requestId,
        })
      },
      sendResult: () => {
        throw new Error(
          "ILDCP never sends packets so it should never receive responses"
        )
      },
    }
  }) satisfies PacketSender<"ildcp">
