import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { processPacket } from "../ilp-connector/process-packet"
import {
  IlpType,
  serializeIlpPacket,
} from "../ilp-connector/schemas/ilp-packet-codec"
import { IldcpEndpointInfo } from "../ilp-connector/senders/send-ildcp-packets"
import { routingTableSignal } from "../routing/signals/routing-table"
import { ildcpResponseSchema } from "./ildcp-packet-codec"

const logger = createLogger("das:node:handle-ildcp-requests")

export const ILDCP_ADDRESS = "peer.config"

export interface IldcpRequestParameters {
  sourceIlpAddress: string
  requestId: number
}

const ILDCP_DESTINATION_INFO: IldcpEndpointInfo = {
  type: "ildcp",
  ilpAddress: ILDCP_ADDRESS,
  accountPath: `internal/ildcp`,
}

export const handleIldcpRequests = () =>
  createActor((sig) => {
    const ilpRoutingTable = sig.get(routingTableSignal)
    const processIncomingPacketActor = sig.use(processPacket)

    ilpRoutingTable.set(ILDCP_ADDRESS, {
      type: "fixed",
      destination: ILDCP_DESTINATION_INFO,
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
          sourceEndpointInfo: ILDCP_DESTINATION_INFO,
          serializedPacket: serializeIlpPacket(responsePacket),
          parsedPacket: responsePacket,
          requestId,
        })
      },
    }
  })
