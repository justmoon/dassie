import {
  SerializableLogLine,
  createCliFormatter,
  createJsonFormatter,
  createLogger,
} from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { logsStore } from "../../common/stores/logs"

export const registerReactiveLogger = () =>
  createActor((sig) => {
    let inRecursion = false
    const cliFormatter = createCliFormatter()
    const jsonFormatter = createJsonFormatter({
      outputFunction(line: SerializableLogLine) {
        sig.use(logsStore).addLogLine({
          node: "",
          ...line,
        })
      },
    })

    createLogger.setFormatter((...parameters) => {
      if (process.env["DASSIE_LOG_TO_CLI"] === "true") {
        cliFormatter(...parameters)
      }
      if (inRecursion) return

      inRecursion = true
      jsonFormatter(...parameters)
      inRecursion = false
    })
  })
