import {
  captureConsole,
  compareLogLevel,
  createCliFormatter,
  context as loggingContext,
} from "@dassie/lib-logger"
import type { Reactor } from "@dassie/lib-reactive"

import { LogsStore } from "../../../common/stores/logs"
import { StdoutLogLevel } from "../signals/stdout-log-level"

export const AttachLogger = (reactor: Reactor) => {
  const logsStore = reactor.use(LogsStore)
  const stdoutLogLevel = reactor.use(StdoutLogLevel)

  return function attachLogger() {
    captureConsole()

    const cliFormatter = createCliFormatter()

    loggingContext.output = (logEvent) => {
      if (logEvent.type === "clear") {
        logsStore.clear()
        return
      }

      logsStore.addLogLine(logEvent)

      const currentLogLevel = stdoutLogLevel.read()
      if (
        currentLogLevel !== "none" &&
        compareLogLevel(logEvent.type, currentLogLevel) >= 0
      ) {
        process.stdout.write(cliFormatter(logEvent) + "\n")
      }
    }
  }
}
