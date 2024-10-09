import type { Reactor } from "@dassie/lib-reactive"
import { tell } from "@dassie/lib-type-utils"

import { SendAsyncPrepare } from "../../ilp-http/functions/send-async-prepare"
import { SendAsyncResult } from "../../ilp-http/functions/send-async-result"
import { IncomingRequestIdMap } from "../../ilp-http/values/incoming-request-id-map"
import type { PacketSender } from "../functions/send-packet"

export interface IlpHttpEndpointInfo {
  readonly type: "http"
  readonly id: string
}

export const SendIlpHttpPackets = (reactor: Reactor): PacketSender<"http"> => {
  const sendAsyncPrepare = reactor.use(SendAsyncPrepare)
  const sendAsyncResult = reactor.use(SendAsyncResult)
  const requestIdMap = reactor.use(IncomingRequestIdMap)

  return {
    sendPrepare: ({ serializedPacket, outgoingRequestId: requestId }) => {
      tell(() =>
        // TODO: Select URL based on id
        sendAsyncPrepare({
          packet: serializedPacket,
          url: "http://localhost:3002",
          requestId: String(requestId),
        }),
      )
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

      tell(() =>
        sendAsyncResult({
          packet: serializedPacket,
          callbackUrl,
          requestId,
        }),
      )
    },
  }
}
