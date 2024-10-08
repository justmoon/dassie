import { type IlpPacket, IlpType } from "@dassie/lib-protocol-ilp"

import type { DassieReactor } from "../../base/types/dassie-base"
import { connector as logger } from "../../logger/instances"
import { GetEndpointIlpAddress } from "./get-endpoint-ilp-address"
import { ProcessFulfillPacket } from "./process-fulfill-packet"
import { ProcessPreparePacket } from "./process-prepare-packet"
import { ProcessRejectPacket } from "./process-reject-packet"
import type { EndpointInfo } from "./send-packet"

export interface ProcessIncomingPacketParameters<
  TType extends IlpType = IlpType,
> {
  sourceEndpointInfo: EndpointInfo
  serializedPacket: Uint8Array
  parsedPacket: Extract<IlpPacket, { type: TType }>
  requestId: number | string
}

export type IlpPacketHandler<TType extends IlpType> = (
  parameters: ProcessIncomingPacketParameters<TType>,
) => void

export type IlpPacketHandlers = {
  [K in IlpType]: IlpPacketHandler<K>
}

export const ProcessPacket = (reactor: DassieReactor) => {
  const getEndpointIlpAddress = reactor.use(GetEndpointIlpAddress)

  const handlers: IlpPacketHandlers = {
    [IlpType.Prepare]: reactor.use(ProcessPreparePacket),
    [IlpType.Fulfill]: reactor.use(ProcessFulfillPacket),
    [IlpType.Reject]: reactor.use(ProcessRejectPacket),
  }

  function processPacket<TType extends IlpType>(
    parameters: ProcessIncomingPacketParameters<TType>,
  ) {
    logger.debug?.("handle interledger packet", {
      from: getEndpointIlpAddress(parameters.sourceEndpointInfo),
    })

    const handler = handlers[
      parameters.parsedPacket.type
    ] as IlpPacketHandler<TType>

    handler(parameters)
  }

  return processPacket
}
