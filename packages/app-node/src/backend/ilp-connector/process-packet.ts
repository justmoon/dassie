import { createActor } from "@dassie/lib-reactive"
import { UnreachableCaseError } from "@dassie/lib-type-utils"

import { connector as logger } from "../logger/instances"
import { ProcessFulfillPacket } from "./functions/process-fulfill-packet"
import { ProcessPreparePacket } from "./functions/process-prepare-packet"
import { ProcessRejectPacket } from "./functions/process-reject-packet"
import { EndpointInfo } from "./functions/send-packet"
import { IlpPacket, IlpType, parseIlpPacket } from "./schemas/ilp-packet-codec"

export interface ProcessIncomingPacketParameters {
  sourceEndpointInfo: EndpointInfo
  serializedPacket: Uint8Array
  parsedPacket?: IlpPacket | undefined
  requestId: number
}

export const ProcessPacketActor = () =>
  createActor((sig) => {
    const processPreparePacket = sig.use(ProcessPreparePacket)
    const processFulfillPacket = sig.use(ProcessFulfillPacket)
    const processRejectPacket = sig.use(ProcessRejectPacket)

    return {
      handle: ({
        sourceEndpointInfo,
        serializedPacket,
        requestId,
        parsedPacket: optionalParsedPacket,
      }: ProcessIncomingPacketParameters) => {
        logger.debug("handle interledger packet", {
          from: sourceEndpointInfo.ilpAddress,
        })

        // Parse packet if not already done
        const parsedPacket =
          optionalParsedPacket ?? parseIlpPacket(serializedPacket)

        switch (parsedPacket.type) {
          case IlpType.Prepare: {
            processPreparePacket({
              sourceEndpointInfo,
              serializedPacket,
              parsedPacket,
              requestId,
            })
            break
          }
          case IlpType.Fulfill: {
            processFulfillPacket({
              serializedPacket,
              parsedPacket,
              requestId,
            })
            break
          }
          case IlpType.Reject: {
            processRejectPacket({
              serializedPacket,
              parsedPacket,
              requestId,
            })
            break
          }
          default: {
            throw new UnreachableCaseError(parsedPacket)
          }
        }
      },
    }
  })
