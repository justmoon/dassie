import { context as loggingContext } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { trpcClientService } from "../services/trpc-client"

export const forwardLogs = () =>
  createActor((sig) => {
    const trpcClient = sig.get(trpcClientService)
    if (!trpcClient) return

    loggingContext.output = (logEvent) => {
      void trpcClient.runner.notifyLogLine.mutate([
        process.env["DASSIE_DEV_NODE_ID"] ?? "unknown",
        logEvent,
      ])
    }
  })
