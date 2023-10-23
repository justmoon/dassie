import { createActor } from "@dassie/lib-reactive"

import { ProcessPacketActor } from "../ilp-connector/process-packet"
import {
  IlpType,
  serializeIlpPacket,
} from "../ilp-connector/schemas/ilp-packet-codec"
import { IldcpEndpointInfo } from "../ilp-connector/senders/send-ildcp-packets"
import { ildcp as logger } from "../logger/instances"
import { RoutingTableSignal } from "../routing/signals/routing-table"
import { ildcpResponseSchema } from "./ildcp-packet-codec"

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

export const HandleIldcpRequestsActor = () =>
  createActor((sig) => {
    const ilpRoutingTable = sig.get(RoutingTableSignal)
    const processIncomingPacketActor = sig.use(ProcessPacketActor)

    ilpRoutingTable.set(ILDCP_ADDRESS, {
      type: "fixed",
      destination: ILDCP_DESTINATION_INFO,
    })

    sig.onCleanup(() => {
      ilpRoutingTable.delete(ILDCP_ADDRESS)
    })

    return sig.handlers({
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

        processIncomingPacketActor.api.handle.tell({
          sourceEndpointInfo: ILDCP_DESTINATION_INFO,
          serializedPacket: serializeIlpPacket(responsePacket),
          parsedPacket: responsePacket,
          requestId,
        })
      },
    })
  })
