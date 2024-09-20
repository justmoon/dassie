import { LogsStore } from "@dassie/app-dassie/src/logger/stores/logs"
import { createActor } from "@dassie/lib-reactive"

import { type RpcReactor } from "../services/rpc-client"

export const ForwardLogsActor = (reactor: RpcReactor) => {
  const logsStore = reactor.use(LogsStore)

  return createActor((sig) => {
    sig.on(logsStore.changes, ([action, parameters]) => {
      if (action === "addLogLine") {
        void reactor.base.rpc.notifyLogLine.mutate({
          ...parameters[0],
          node: process.env["DASSIE_DEV_NODE_ID"] ?? "unknown",
        })
      }
    })
  })
}
