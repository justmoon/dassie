import { createLogger } from "@dassie/lib-logger"
import type { EffectContext } from "@dassie/lib-reactive"

import { IlpType } from "../ilp-connector/ilp-packet-codec"
import { ilpRoutingTableSignal } from "../ilp-connector/signals/ilp-routing-table"
import { outgoingIlpPacketBuffer } from "../ilp-connector/topics/outgoing-ilp-packet"
import { ildcpResponseSchema } from "./ildcp-packet-codec"

const logger = createLogger("das:node:handle-ildcp-requests")

export const ILDCP_ADDRESS = "peer.config"

export const handleIldcpRequests = (sig: EffectContext) => {
  const ilpRoutingTable = sig.get(ilpRoutingTableSignal)

  ilpRoutingTable.set(ILDCP_ADDRESS, {
    prefix: ILDCP_ADDRESS,
    type: "peer",
    sendPacket: ({ source, requestId }) => {
      // IL-DCP
      const ildcpSerializationResult = ildcpResponseSchema.serialize({
        address: source,
        assetScale: 9,
        assetCode: "XRP",
      })

      if (!ildcpSerializationResult.success) {
        logger.debug("failed to serialize IL-DCP response", {
          error: ildcpSerializationResult.error,
        })
        return
      }

      const responsePacket = {
        type: IlpType.Fulfill,
        fulfillment: new Uint8Array(32),
        data: ildcpSerializationResult.value,
      }

      logger.debug("sending IL-DCP response", {
        destination: source,
      })

      sig.use(outgoingIlpPacketBuffer).emitEvent({
        source: ILDCP_ADDRESS,
        packet: responsePacket,
        incomingRequestId: requestId,
        outgoingRequestId: requestId,
      })
    },
  })
}
