import { createLogger } from "@dassie/lib-logger"

import { nodeIlpAddressSignal } from "../computed/node-ilp-address"
import { IlpErrorCode } from "../schemas/ilp-errors"
import {
  IlpRejectPacket,
  IlpType,
  serializeIlpPacket,
} from "../schemas/ilp-packet-codec"
import { createProcessRejectPacket } from "./process-reject-packet"

const logger = createLogger("das:ilp-connector:trigger-rejection")

export interface RejectPacketEnvironment {
  ownIlpAddress: ReturnType<typeof nodeIlpAddressSignal>
  processRejectPacket: ReturnType<typeof createProcessRejectPacket>
}

export interface RejectPacketParameters {
  requestId: number
  errorCode: IlpErrorCode
  message: string
}

export const createTriggerRejection = ({
  ownIlpAddress,
  processRejectPacket,
}: RejectPacketEnvironment) => {
  return ({ requestId, errorCode, message }: RejectPacketParameters) => {
    logger.info("triggering ILP rejection", { requestId, errorCode, message })

    const rejectPacket: IlpRejectPacket & { type: typeof IlpType.Reject } = {
      type: IlpType.Reject,
      code: errorCode,
      triggeredBy: ownIlpAddress.read(),
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
