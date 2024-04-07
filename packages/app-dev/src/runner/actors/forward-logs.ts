import { LogsStore } from "@dassie/app-node/src/common/stores/logs"
import { type Reactor, createActor } from "@dassie/lib-reactive"

import { RpcClientServiceActor } from "../services/rpc-client"

export const ForwardLogsActor = (reactor: Reactor) => {
  const logsStore = reactor.use(LogsStore)

  return createActor((sig) => {
    const rpcClient = sig.readAndTrack(RpcClientServiceActor)
    if (!rpcClient) return

    sig.on(logsStore.changes, ([action, parameters]) => {
      if (action === "addLogLine") {
        void rpcClient.notifyLogLine.mutate(parameters[0])
      }
    })
  })
}
