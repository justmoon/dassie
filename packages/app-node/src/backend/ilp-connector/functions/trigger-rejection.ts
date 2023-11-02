import { Reactor } from "@dassie/lib-reactive"

import { connector as logger } from "../../logger/instances"
import { NodeIlpAddressSignal } from "../computed/node-ilp-address"
import { IlpErrorCode } from "../schemas/ilp-errors"
import {
  IlpRejectPacket,
  IlpType,
  serializeIlpPacket,
} from "../schemas/ilp-packet-codec"
import { ProcessRejectPacket } from "./process-reject-packet"

export interface RejectPacketParameters {
  requestId: number
  errorCode: IlpErrorCode
  message: string
}

export const TriggerRejection = (reactor: Reactor) => {
  const nodeIlpAddressSignal = reactor.use(NodeIlpAddressSignal)
  const processRejectPacket = reactor.use(ProcessRejectPacket)

  return ({ requestId, errorCode, message }: RejectPacketParameters) => {
    logger.info("triggering ILP rejection", { requestId, errorCode, message })

    const rejectPacket: IlpRejectPacket & { type: typeof IlpType.Reject } = {
      type: IlpType.Reject,
      code: errorCode,
      triggeredBy: nodeIlpAddressSignal.read(),
      message,
      data: Buffer.alloc(0),
    }

    const serializedPacket = serializeIlpPacket(rejectPacket)

    processRejectPacket({
      parsedPacket: rejectPacket,
      serializedPacket,
      requestId,
    })
  }
}
