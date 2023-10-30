import { UnreachableCaseError } from "@dassie/lib-type-utils"

import { getLogContext } from "./context"
import { createEnableChecker } from "./enabled"

export * from "./common"

export const context = getLogContext()

context.enableChecker = createEnableChecker("")
context.output = (logEvent, context) => {
  switch (logEvent.type) {
    case "debug":
    case "info":
    case "warn":
    case "error": {
      context.realConsole[logEvent.type](
        `${logEvent.namespace} ${logEvent.message}`,
        ...logEvent.parameters,
      )
      break
    }
    case "clear": {
      context.realConsole.clear()
      break
    }
    default: {
      throw new UnreachableCaseError(logEvent)
    }
  }
}
