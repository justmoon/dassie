import type { Reactor } from "@dassie/lib-reactive"

import { HandleIldcpRequestsActor } from "../../ildcp-server/handle-ildcp-requests"
import { GetEndpointIlpAddress } from "../functions/get-endpoint-ilp-address"
import type { PacketSender } from "../functions/send-packet"

export interface IldcpEndpointInfo {
  readonly type: "ildcp"
}

export const SendIldcpPackets = (reactor: Reactor): PacketSender<"ildcp"> => {
  const ildcpHandler = reactor.use(HandleIldcpRequestsActor)
  const getEndpointIlpAddress = reactor.use(GetEndpointIlpAddress)

  return {
    sendPrepare: ({ sourceEndpointInfo, outgoingRequestId: requestId }) => {
      ildcpHandler.api.handle.tell({
        sourceIlpAddress: getEndpointIlpAddress(sourceEndpointInfo),
        requestId,
      })
    },

    sendResult: () => {
      throw new Error(
        "ILDCP never sends packets so it should never receive responses",
      )
    },
  }
}
