import {
  captureConsole,
  createCliFormatter,
  context as loggingContext,
} from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { logsStore } from "../../common/stores/logs"

export const registerReactiveLogger = () =>
  createActor((sig) => {
    const logs = sig.use(logsStore)

    captureConsole()

    const cliFormatter = createCliFormatter()

    loggingContext.output = (logEvent) => {
      if (logEvent.type === "clear") {
        logs.clear()
      } else {
        logs.addLogLine({
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
