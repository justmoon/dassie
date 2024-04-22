import { DassieReactor } from "../../base/types/dassie-base"
import { connector as logger } from "../../logger/instances"
import { IlpPacket, IlpType } from "../schemas/ilp-packet-codec"
import { ProcessFulfillPacket } from "./process-fulfill-packet"
import { ProcessPreparePacket } from "./process-prepare-packet"
import { ProcessRejectPacket } from "./process-reject-packet"
import { EndpointInfo } from "./send-packet"

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
  const handlers: IlpPacketHandlers = {
    [IlpType.Prepare]: reactor.use(ProcessPreparePacket),
    [IlpType.Fulfill]: reactor.use(ProcessFulfillPacket),
    [IlpType.Reject]: reactor.use(ProcessRejectPacket),
  }

  function processPacket<TType extends IlpType>(
    parameters: ProcessIncomingPacketParameters<TType>,
  ) {
    logger.debug?.("handle interledger packet", {
      from: parameters.sourceEndpointInfo.ilpAddress,
    })

    const handler = handlers[
      parameters.parsedPacket.type
    ] as IlpPacketHandler<TType>

    handler(parameters)
  }

  return processPacket
}
