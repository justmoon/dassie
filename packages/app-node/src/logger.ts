import { createLogger } from "@dassie/lib-logger"

export const attachLogger = () => {
  const logger = createLogger("console")
  logger.captureConsole()
}
