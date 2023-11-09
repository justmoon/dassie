import { Reactor } from "@dassie/lib-reactive"

import { SendIlpHttpPacketsActor } from "../../ilp-http/send-ilp-http-packets"
import { IncomingRequestIdMap } from "../../ilp-http/values/incoming-request-id-map"
import { CommonEndpointInfo, PacketSender } from "../functions/send-packet"

export interface IlpHttpEndpointInfo extends CommonEndpointInfo {
  readonly type: "http"
}

export const SendIlpHttpPackets = (reactor: Reactor): PacketSender<"http"> => {
  const sendIlpHttpPacketsActor = reactor.use(SendIlpHttpPacketsActor)
  const requestIdMap = reactor.use(IncomingRequestIdMap)

  return {
    sendPrepare: ({ serializedPacket, outgoingRequestId: requestId }) => {
      sendIlpHttpPacketsActor.api.sendAsyncPrepare.tell({
        packet: serializedPacket,
        url: "http://localhost:3002",
        requestId: String(requestId),
      })
    },

    sendResult: ({
      serializedPacket,
      prepare: { incomingRequestId: numericRequestId },
    }) => {
      const entry = requestIdMap.get(numericRequestId)

      if (!entry) {
        throw new Error("Invalid request id")
      }

      const { callbackUrl, requestId } = entry

      sendIlpHttpPacketsActor.api.sendAsyncResult.tell({
        packet: serializedPacket,
        callbackUrl,
        requestId,
      })
    },
  }
}
