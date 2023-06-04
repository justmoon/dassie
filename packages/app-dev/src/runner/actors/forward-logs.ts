import { attachLogger } from "@dassie/app-node/src/backend/logger"
import {
  type SerializableLogEvent,
  captureConsole,
  createJsonFormatter,
} from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { trpcClientService } from "../services/trpc-client"

export const forwardLogs = () =>
  createActor((sig) => {
    const trpcClient = sig.get(trpcClientService)
    if (!trpcClient) return

    const jsonFormatter = createJsonFormatter({
      serializationFunction(line: SerializableLogEvent) {
        void trpcClient.runner.notifyLogLine.mutate([
          process.env["DASSIE_DEV_NODE_ID"] ?? "unknown",
          line,
        ])
        return ""
      },
    })

    // Overwrite logger behavior so we can forward logs to the server
    sig.use(attachLogger).behavior = () => {
      captureConsole({ formatter: jsonFormatter, outputFunction: () => void 0 })
    }
  })
