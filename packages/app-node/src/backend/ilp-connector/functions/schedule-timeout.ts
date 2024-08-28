import { isError } from "@dassie/lib-logger"
import { IlpErrorCode } from "@dassie/lib-protocol-ilp"

import { DassieReactor } from "../../base/types/dassie-base"
import { connector as logger } from "../../logger/instances"
import { EndpointInfo } from "./send-packet"
import { TriggerLateRejection } from "./trigger-late-rejection"

export interface ScheduleTimeoutParameters {
  sourceEndpointInfo: EndpointInfo
  requestId: number
  timeoutAbort: AbortController
}

export const ScheduleTimeout = (reactor: DassieReactor) => {
  const triggerLateRejection = reactor.use(TriggerLateRejection)

  function scheduleTimeout({
    sourceEndpointInfo,
    requestId,
    timeoutAbort,
  }: ScheduleTimeoutParameters) {
    reactor.base.time
      .timeout(5000, { signal: timeoutAbort.signal })
      .then(() => {
        logger.debug?.("ILP packet timed out", { requestId })
        triggerLateRejection({
          sourceEndpointInfo,
          requestId,
          errorCode: IlpErrorCode.R00_TRANSFER_TIMED_OUT,
          message: "Packet timed out",
        })
      })
      .catch((error: unknown) => {
        if (isError(error) && error.name === "AbortError") return

        logger.error("error in packet timeout handler", { error })
      })
  }

  return scheduleTimeout
}
