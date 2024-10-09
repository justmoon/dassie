import { ILDCP_ADDRESS, ildcpResponseSchema } from "@dassie/lib-protocol-ildcp"
import { IlpType, serializeIlpPacket } from "@dassie/lib-protocol-ilp"
import { createActor } from "@dassie/lib-reactive"
import { isFailure } from "@dassie/lib-type-utils"

import type { DassieActorContext } from "../base/types/dassie-base"
import { ProcessPacket } from "../ilp-connector/functions/process-packet"
import type { IldcpEndpointInfo } from "../ilp-connector/senders/send-ildcp-packets"
import { ildcp as logger } from "../logger/instances"
import { RoutingTableSignal } from "../routing/signals/routing-table"

export interface IldcpRequestParameters {
  sourceIlpAddress: string
  requestId: number
}

export const HandleIldcpRequestsActor = () =>
  createActor((sig: DassieActorContext) => {
    const ilpRoutingTable = sig.read(RoutingTableSignal)
    const processPacket = sig.reactor.use(ProcessPacket)

    const ildcpDestinationInfo: IldcpEndpointInfo = {
      type: "ildcp",
    }

    ilpRoutingTable.set(ILDCP_ADDRESS, {
      type: "fixed",
      destination: ildcpDestinationInfo,
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

        if (isFailure(ildcpSerializationResult)) {
          logger.debug?.("failed to serialize IL-DCP response", {
            error: ildcpSerializationResult,
          })
          return
        }

        const responsePacket = {
          type: IlpType.Fulfill,
          data: {
            fulfillment: new Uint8Array(32),
            data: ildcpSerializationResult,
          },
        }

        logger.debug?.("sending IL-DCP response", {
          destination: sourceIlpAddress,
        })

        processPacket({
          sourceEndpointInfo: ildcpDestinationInfo,
          serializedPacket: serializeIlpPacket(responsePacket),
          parsedPacket: responsePacket,
          requestId,
        })
      },
    }
  })
