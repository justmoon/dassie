import {
  SerializableLogLine,
  createJsonFormatter,
  createLogger,
} from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { trpcClientService } from "../services/trpc-client"

export const forwardLogs = () =>
  createActor((sig) => {
    const trpcClient = sig.get(trpcClientService)
    if (!trpcClient) return

    let inRecursion = false
    const jsonFormatter = createJsonFormatter({
      outputFunction(line: SerializableLogLine) {
        void trpcClient.runner.notifyLogLine.mutate([
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
  })
