import { createLogger } from "@xen-ilp/lib-logger"

export const attachLogger = () => {
  const logger = createLogger("console")
  logger.captureConsole()
}
