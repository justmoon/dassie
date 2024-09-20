import {
  IlpErrorCode,
  type IlpPacket,
  IlpType,
  serializeIlpPacket,
} from "@dassie/lib-protocol-ilp"

import type { DassieReactor } from "../../base/types/dassie-base"
import { connector as logger } from "../../logger/instances"
import { NodeIlpAddressSignal } from "../computed/node-ilp-address"
import { ProcessRejectPacket } from "./process-reject-packet"
import type { EndpointInfo } from "./send-packet"

export interface LateRejectionParameters {
  sourceEndpointInfo: EndpointInfo
  requestId: number
  errorCode: IlpErrorCode
  message: string
}

export const TriggerLateRejection = (reactor: DassieReactor) => {
  const nodeIlpAddressSignal = reactor.use(NodeIlpAddressSignal)
  const processRejectPacket = reactor.use(ProcessRejectPacket)

  function triggerLateRejection({
    sourceEndpointInfo,
    requestId,
    errorCode,
    message,
  }: LateRejectionParameters) {
    logger.info("triggering ILP rejection", { requestId, errorCode, message })

    const rejectPacket: IlpPacket & { type: typeof IlpType.Reject } = {
      type: IlpType.Reject,
      data: {
        code: errorCode,
        triggeredBy: nodeIlpAddressSignal.read(),
        message,
        data: Buffer.alloc(0),
      },
    }

    const serializedPacket = serializeIlpPacket(rejectPacket)

    processRejectPacket({
      sourceEndpointInfo,
      parsedPacket: rejectPacket,
      serializedPacket,
      requestId,
    })
  }

  return triggerLateRejection
}
