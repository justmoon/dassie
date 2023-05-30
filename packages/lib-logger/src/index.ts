import { createEnableChecker } from "./enabled"
import { createLoggerFactory } from "./logger"

export * from "./common"

/*
 * Factory for Node.js CLI loggers.
 *
 * @beta
 */
export const createLogger = createLoggerFactory({
  enableChecker: createEnableChecker(process.env["DEBUG"] ?? ""),
})
