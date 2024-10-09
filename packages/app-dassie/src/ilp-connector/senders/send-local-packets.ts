import { serializeIlpPacket } from "@dassie/lib-protocol-ilp"
import { tell } from "@dassie/lib-type-utils"

import type { DassieReactor } from "../../base/types/dassie-base"
import { LocalEndpointsStore } from "../../local-ilp/stores/local-endpoints"
import { OutstandingRequestsStore } from "../../local-ilp/stores/outstanding-requests"
import { ProcessPacket } from "../functions/process-packet"
import type { PacketSender } from "../functions/send-packet"

export interface LocalEndpointInfo {
  readonly type: "local"
  readonly hint: string
  readonly localIlpAddressPart: string
}

export const SendLocalPackets = (
  reactor: DassieReactor,
): PacketSender<"local"> => {
  const outstandingRequestsStore = reactor.use(OutstandingRequestsStore)
  const localEndpointsStore = reactor.use(LocalEndpointsStore)

  return {
    sendPrepare: ({
      destinationEndpointInfo,
      parsedPacket,
      outgoingRequestId,
    }) => {
      tell(async () => {
        const prepareCallback = localEndpointsStore
          .read()
          .get(destinationEndpointInfo.localIlpAddressPart)?.prepareCallback

        if (!prepareCallback) {
          // TODO: Reject packet without throwing an error
          throw new Error("No prepare callback")
        }
        const result = await prepareCallback(parsedPacket.data)

        // Late use() to avoid circular dependency
        const processPacket = reactor.use(ProcessPacket)
        processPacket({
          sourceEndpointInfo: destinationEndpointInfo,
          serializedPacket: serializeIlpPacket(result),
          parsedPacket: result,
          requestId: outgoingRequestId,
        })
      })
    },
    sendResult: ({
      prepare: { incomingRequestId: requestId },
      parsedPacket,
    }) => {
      const resolve = outstandingRequestsStore.read().get(requestId)

      if (!resolve) {
        throw new Error("Invalid request ID")
      }

      outstandingRequestsStore.act.removeRequest(requestId)

      resolve(parsedPacket)
    },
  }
}
