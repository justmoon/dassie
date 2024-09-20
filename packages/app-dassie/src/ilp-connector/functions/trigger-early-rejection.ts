import {
  type IlpPacket,
  IlpType,
  serializeIlpPacket,
} from "@dassie/lib-protocol-ilp"

import type { DassieReactor } from "../../base/types/dassie-base"
import { connector as logger } from "../../logger/instances"
import { NodeIlpAddressSignal } from "../computed/node-ilp-address"
import type { PreparedIlpPacketEvent } from "../topics/prepared-ilp-packet"
import type { IlpFailure } from "../types/ilp-failure"
import { SendPacket } from "./send-packet"

export interface EarlyRejectionParameters {
  prepare: PreparedIlpPacketEvent
  failure: IlpFailure
}

export const TriggerEarlyRejection = (reactor: DassieReactor) => {
  const nodeIlpAddressSignal = reactor.use(NodeIlpAddressSignal)
  const sendPacket = reactor.use(SendPacket)

  function triggerEarlyRejection({
    prepare,
    failure,
  }: EarlyRejectionParameters) {
    logger.info("triggering ILP rejection", {
      requestId: prepare.incomingRequestId,
      errorCode: failure.errorCode,
      message: failure.message,
    })

    const rejectPacket: IlpPacket & { type: typeof IlpType.Reject } = {
      type: IlpType.Reject,
      data: {
        code: failure.errorCode,
        triggeredBy: nodeIlpAddressSignal.read(),
        message: failure.message,
        data: Buffer.alloc(0),
      },
    }

    const serializedPacket = serializeIlpPacket(rejectPacket)

    sendPacket({
      prepare,
      serializedPacket,
      parsedPacket: rejectPacket,
      destinationEndpointInfo: prepare.sourceEndpointInfo,
    })
  }

  return triggerEarlyRejection
}
