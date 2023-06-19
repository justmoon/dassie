import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { IlpType, serializeIlpPacket } from "../ilp-connector/ilp-packet-codec"
import { processPacket } from "../ilp-connector/process-packet"
import { routingTableSignal } from "../ilp-connector/signals/routing-table"
import { ildcpResponseSchema } from "./ildcp-packet-codec"

const logger = createLogger("das:node:handle-ildcp-requests")

export const ILDCP_ADDRESS = "peer.config"

export interface IldcpRequestParameters {
  sourceIlpAddress: string
  requestId: number
}

export const handleIldcpRequests = () =>
  createActor((sig) => {
    const ilpRoutingTable = sig.get(routingTableSignal)
    const processIncomingPacketActor = sig.use(processPacket)

    ilpRoutingTable.set(ILDCP_ADDRESS, {
      type: "ildcp",
    })

    sig.onCleanup(() => {
      ilpRoutingTable.delete(ILDCP_ADDRESS)
    })

    return {
      handle: ({ sourceIlpAddress, requestId }: IldcpRequestParameters) => {
        // IL-DCP
        const ildcpSerializationResult = ildcpResponseSchema.serialize({
          address: sourceIlpAddress,
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
          destination: sourceIlpAddress,
        })

        processIncomingPacketActor.tell("handle", {
          sourceIlpAddress: ILDCP_ADDRESS,
          ledgerAccountPath: `internal/ildcp`,
          serializedPacket: serializeIlpPacket(responsePacket),
          parsedPacket: responsePacket,
          requestId,
        })
      },
    }
  })
