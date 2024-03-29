import { captureConsole } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

export const AttachLoggerActor = () =>
  createActor(() => {
    captureConsole()
  })
