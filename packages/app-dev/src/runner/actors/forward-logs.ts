import { LogsStore } from "@dassie/app-node/src/common/stores/logs"
import { type Reactor, createActor } from "@dassie/lib-reactive"

import { RpcClientServiceActor } from "../services/rpc-client"

export const ForwardLogsActor = (reactor: Reactor) => {
  const logsStore = reactor.use(LogsStore)

  return createActor((sig) => {
    const trpcClient = sig.readAndTrack(RpcClientServiceActor)
    if (!trpcClient) return

    sig.on(logsStore.changes, ([action, parameters]) => {
      if (action === "addLogLine") {
        void trpcClient.runner.notifyLogLine.mutate(parameters[0])
      }
    })
  })
}
