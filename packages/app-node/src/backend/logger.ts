import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

export const attachLogger = () =>
  createActor(() => {
    const logger = createLogger("console")
    logger.captureConsole()
  })
