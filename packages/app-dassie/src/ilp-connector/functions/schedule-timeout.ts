import { IlpErrorCode } from "@dassie/lib-protocol-ilp"
import { delayWithAbortSignal } from "@dassie/lib-reactive"
import { isError, isFailure } from "@dassie/lib-type-utils"

import type { DassieReactor } from "../../base/types/dassie-base"
import { connector as logger } from "../../logger/instances"
import type { EndpointInfo } from "./send-packet"
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
    delayWithAbortSignal(reactor.base.clock, 5000, timeoutAbort.signal)
      .then((value) => {
        if (isFailure(value)) return

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
