import { createActor } from "@dassie/lib-reactive"

import { handleIldcpRequests } from "../../ildcp-server/handle-ildcp-requests"
import { PacketSender } from "../send-outgoing-packets"

export interface IldcpDestinationInfo {
  type: "ildcp"
}

export const sendIldcpPackets = () =>
  createActor((sig) => {
    const ildcpHandler = sig.use(handleIldcpRequests)

    return {
      sendPrepare: ({ sourceIlpAddress, outgoingRequestId: requestId }) => {
        ildcpHandler.tell("handle", {
          sourceIlpAddress,
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
