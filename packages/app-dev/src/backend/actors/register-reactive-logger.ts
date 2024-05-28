import {
  captureConsole,
  createCliFormatter,
  context as loggingContext,
} from "@dassie/lib-logger"
import { Reactor, createActor } from "@dassie/lib-reactive"

import { LogsStore } from "../../common/stores/logs"

export const RegisterReactiveLoggerActor = (reactor: Reactor) => {
  const logsStore = reactor.use(LogsStore)
  return createActor(() => {
    captureConsole()

    const cliFormatter = createCliFormatter()

    loggingContext.output = (logEvent) => {
      if (logEvent.type === "clear") {
        logsStore.act.clear()
      } else {
        logsStore.act.addLogLine({
          node: "host",
          ...logEvent,
        })
      }

      if (
        process.env["DASSIE_LOG_TO_CLI"] === "true" ||
        logEvent.type === "error" ||
        logEvent.type === "warn"
      ) {
        process.stdout.write(cliFormatter(logEvent))
        process.stdout.write("\n")
      }
    }
  })
}
