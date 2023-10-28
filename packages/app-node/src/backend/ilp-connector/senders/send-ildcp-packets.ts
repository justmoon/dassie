import { createActor } from "@dassie/lib-reactive"

import { HandleIldcpRequestsActor } from "../../ildcp-server/handle-ildcp-requests"
import { CommonEndpointInfo, PacketSender } from "../functions/send-packet"

export interface IldcpEndpointInfo extends CommonEndpointInfo {
  readonly type: "ildcp"
}

export const SendIldcpPacketsActor = () =>
  createActor((sig) => {
    const ildcpHandler = sig.use(HandleIldcpRequestsActor)

    return {
      sendPrepare: ({ sourceEndpointInfo, outgoingRequestId: requestId }) => {
        ildcpHandler.api.handle.tell({
          sourceIlpAddress: sourceEndpointInfo.ilpAddress,
          requestId,
        })
      },

      sendResult: () => {
        throw new Error(
          "ILDCP never sends packets so it should never receive responses",
        )
      },
    }
  }) satisfies PacketSender<"ildcp">
