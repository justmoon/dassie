import { setTimeout } from "node:timers/promises"

import { isError } from "@dassie/lib-logger"
import { Reactor } from "@dassie/lib-reactive"

import { connector as logger } from "../../logger/instances"
import { IlpErrorCode } from "../schemas/ilp-errors"
import { TriggerRejection } from "./trigger-rejection"

export interface ScheduleTimeoutParameters {
  requestId: number
  timeoutAbort: AbortController
}

export const ScheduleTimeout = (reactor: Reactor) => {
  const triggerRejection = reactor.use(TriggerRejection)

  return ({ requestId, timeoutAbort }: ScheduleTimeoutParameters) => {
    setTimeout(5000, undefined, { signal: timeoutAbort.signal })
      .then(() => {
        logger.debug("ILP packet timed out", { requestId })
        triggerRejection({
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
