import { LogsStore } from "@dassie/app-node/src/common/stores/logs"
import { createActor } from "@dassie/lib-reactive"

import { type RpcReactor } from "../services/rpc-client"

export const ForwardLogsActor = (reactor: RpcReactor) => {
  const logsStore = reactor.use(LogsStore)

  return createActor((sig) => {
    sig.on(logsStore.changes, ([action, parameters]) => {
      if (action === "addLogLine") {
        void reactor.base.rpc.notifyLogLine.mutate(parameters[0])
      }
    })
  })
}
