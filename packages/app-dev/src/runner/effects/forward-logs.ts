import {
  SerializableLogLine,
  createJsonFormatter,
  createLogger,
} from "@dassie/lib-logger"
import type { EffectContext } from "@dassie/lib-reactive"

import { trpcClientFactory } from "../services/trpc-client"

export const forwardLogs = (sig: EffectContext) => {
  const trpcClient = sig.use(trpcClientFactory)

  let inRecursion = false
  const jsonFormatter = createJsonFormatter({
    outputFunction(line: SerializableLogLine) {
      void trpcClient.mutation("runner.notifyLogLine", [
        process.env["DASSIE_DEV_NODE_ID"] ?? "unknown",
        line,
      ])
    },
  })

  createLogger.setFormatter((...parameters) => {
    if (inRecursion) return

    inRecursion = true
    jsonFormatter(...parameters)
    inRecursion = false
  })
}
