import { createActor } from "@dassie/lib-reactive"

import { AttachLoggerActor } from "./attach-logger"
import { CaptureLogsActor } from "./capture-logs"

export const LoggerActor = () =>
  createActor((sig) => {
    sig.run(AttachLoggerActor)
    sig.run(CaptureLogsActor)
  })
