import { setTimeout } from "node:timers/promises"

import { isError } from "@dassie/lib-logger"
import { Reactor } from "@dassie/lib-reactive"

import { connector as logger } from "../../logger/instances"
import { IlpErrorCode } from "../schemas/ilp-errors"
import { EndpointInfo } from "./send-packet"
import { TriggerLateRejection } from "./trigger-late-rejection"

export interface ScheduleTimeoutParameters {
  sourceEndpointInfo: EndpointInfo
  requestId: number
  timeoutAbort: AbortController
}

export const ScheduleTimeout = (reactor: Reactor) => {
  const triggerLateRejection = reactor.use(TriggerLateRejection)

  return ({
    sourceEndpointInfo,
    requestId,
    timeoutAbort,
  }: ScheduleTimeoutParameters) => {
    setTimeout(5000, undefined, { signal: timeoutAbort.signal })
      .then(() => {
        logger.debug("ILP packet timed out", { requestId })
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
}
