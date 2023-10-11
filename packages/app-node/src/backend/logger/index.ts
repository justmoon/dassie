import { createActor } from "@dassie/lib-reactive"

import { AttachLoggerActor } from "./attach-logger"

export const LoggerActor = () =>
  createActor((sig) => {
    sig.run(AttachLoggerActor)
  })
