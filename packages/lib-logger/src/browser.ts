import { createEnableChecker } from "./enabled"
import { createLoggerFactory } from "./logger"

export * from "./common"

/**
 * Factory for Browser loggers.
 *
 * @beta
 */
export const createLogger = createLoggerFactory({
  enableChecker: createEnableChecker(""),
})
