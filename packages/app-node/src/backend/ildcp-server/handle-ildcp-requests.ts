import { createActor } from "@dassie/lib-reactive"
import { isFailure } from "@dassie/lib-type-utils"

import { AccountPath } from "../accounting/types/account-paths"
import { DassieActorContext } from "../base/types/dassie-base"
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

export const HandleIldcpRequestsActor = () =>
  createActor((sig: DassieActorContext) => {
    const ilpRoutingTable = sig.read(RoutingTableSignal)
    const processIncomingPacketActor = sig.reactor.use(ProcessPacketActor)

    const ildcpDestinationInfo: IldcpEndpointInfo = {
      type: "ildcp",
      ilpAddress: ILDCP_ADDRESS,
      // ILDCP cannot send or receive money so we set the account path to an impossible value
      accountPath: "unreachable" as AccountPath,
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
          logger.debug("failed to serialize IL-DCP response", {
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

        logger.debug("sending IL-DCP response", {
          destination: sourceIlpAddress,
        })

        processIncomingPacketActor.api.handle.tell({
          sourceEndpointInfo: ildcpDestinationInfo,
          serializedPacket: serializeIlpPacket(responsePacket),
          parsedPacket: responsePacket,
          requestId,
        })
      },
    }
  })
