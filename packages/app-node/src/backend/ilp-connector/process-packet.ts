import { createActor } from "@dassie/lib-reactive"

import { DassieReactor } from "../base/types/dassie-base"
import { connector as logger } from "../logger/instances"
import { ProcessFulfillPacket } from "./functions/process-fulfill-packet"
import { ProcessPreparePacket } from "./functions/process-prepare-packet"
import { ProcessRejectPacket } from "./functions/process-reject-packet"
import { EndpointInfo } from "./functions/send-packet"
import { IlpPacket, IlpType, parseIlpPacket } from "./schemas/ilp-packet-codec"

export interface ProcessIncomingPacketParameters<
  TType extends IlpType = IlpType,
> {
  sourceEndpointInfo: EndpointInfo
  serializedPacket: Uint8Array
  parsedPacket: Extract<IlpPacket, { type: TType }>
  requestId: number
}

export type IlpPacketHandler<TType extends IlpType> = (
  parameters: ProcessIncomingPacketParameters<TType>,
) => void

export type IlpPacketHandlers = {
  [K in IlpType]: IlpPacketHandler<K>
}

export const ProcessPacketActor = (reactor: DassieReactor) => {
  const handlers: IlpPacketHandlers = {
    [IlpType.Prepare]: reactor.use(ProcessPreparePacket),
    [IlpType.Fulfill]: reactor.use(ProcessFulfillPacket),
    [IlpType.Reject]: reactor.use(ProcessRejectPacket),
  }

  return createActor(() => {
    const api = {
      parseAndHandle: (
        parameters: Omit<ProcessIncomingPacketParameters, "parsedPacket">,
      ) => {
        const parsedPacket = parseIlpPacket(parameters.serializedPacket)
        return api.handle({ ...parameters, parsedPacket })
      },
      handle: <TType extends IlpType>(
        parameters: ProcessIncomingPacketParameters<TType>,
      ) => {
        logger.debug("handle interledger packet", {
          from: parameters.sourceEndpointInfo.ilpAddress,
        })

        const handler = handlers[
          parameters.parsedPacket.type
        ] as IlpPacketHandler<TType>

        handler(parameters)
      },
    }
    return api
  })
}
