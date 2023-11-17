import { context as loggingContext } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { TrpcClientServiceActor } from "../services/trpc-client"

export const ForwardLogsActor = () =>
  createActor((sig) => {
    const trpcClient = sig.readAndTrack(TrpcClientServiceActor)
    if (!trpcClient) return

    loggingContext.output = (logEvent) => {
      void trpcClient.runner.notifyLogLine.mutate([
        process.env["DASSIE_DEV_NODE_ID"] ?? "unknown",
        logEvent,
      ])
    }
  })
