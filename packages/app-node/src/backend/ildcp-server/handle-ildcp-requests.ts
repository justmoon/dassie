import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { IlpType, serializeIlpPacket } from "../ilp-connector/ilp-packet-codec"
import { processIncomingPacket } from "../ilp-connector/process-incoming-packet"
import { globalIlpRoutingTableSignal } from "../ilp-connector/signals/global-ilp-routing-table"
import { ildcpResponseSchema } from "./ildcp-packet-codec"

const logger = createLogger("das:node:handle-ildcp-requests")

export const ILDCP_ADDRESS = "peer.config"

export const handleIldcpRequests = () =>
  createActor((sig) => {
    const ilpRoutingTable = sig.get(globalIlpRoutingTableSignal)
    const processIncomingPacketActor = sig.use(processIncomingPacket)

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

        processIncomingPacketActor.tell("handle", {
          sourceIlpAddress: ILDCP_ADDRESS,
          ledgerAccountPath: `internal/ildcp`,
          serializedPacket: serializeIlpPacket(responsePacket),
          parsedPacket: responsePacket,
          requestId,
        })
      },
    })

    sig.onCleanup(() => {
      ilpRoutingTable.delete(ILDCP_ADDRESS)
    })
  })
