import {
  SerializableLogLine,
  createCliFormatter,
  createJsonFormatter,
  createLogger,
} from "@xen-ilp/lib-logger"
import type { EffectContext } from "@xen-ilp/lib-reactive"

import { logLineTopic } from "../features/logs"

export const registerReactiveLogger = (sig: EffectContext) => {
  let inRecursion = false
  const cliFormatter = createCliFormatter()
  const jsonFormatter = createJsonFormatter({
    outputFunction(line: SerializableLogLine) {
      sig.reactor.emit(logLineTopic, {
        node: "",
        ...line,
      })
    },
  })

  createLogger.setFormatter((...parameters) => {
    if (process.env["XEN_LOG_TO_CLI"] === "true") {
      cliFormatter(...parameters)
    }
    if (inRecursion) return

    inRecursion = true
    jsonFormatter(...parameters)
    inRecursion = false
  })
}
