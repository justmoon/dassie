import { context as loggingContext } from "@dassie/lib-logger"
import { type Reactor, createActor } from "@dassie/lib-reactive"

import { LogsStore } from "../../common/stores/logs"
import { HasNodeIdentitySignal } from "../config/computed/has-node-identity"
import { NodeIdSignal } from "../ilp-connector/computed/node-id"

export const CaptureLogsActor = (reactor: Reactor) => {
  const logsStore = reactor.use(LogsStore)

  return createActor((sig) => {
    const hasNodeIdentity = sig.readAndTrack(HasNodeIdentitySignal)
    const nodeId = hasNodeIdentity ? sig.readAndTrack(NodeIdSignal) : "none"

    loggingContext.output = (logEvent) => {
      if (logEvent.type === "clear") {
        logsStore.clear()
        return
      }

      logsStore.addLogLine({ ...logEvent, node: nodeId })
    }
  })
}
