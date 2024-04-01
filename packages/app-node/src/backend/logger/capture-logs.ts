import { context as loggingContext } from "@dassie/lib-logger"
import { type Reactor, createActor } from "@dassie/lib-reactive"

import { LogsStore } from "../../common/stores/logs"
import { NodeIdSignal } from "../ilp-connector/computed/node-id"

export const CaptureLogsActor = (reactor: Reactor) => {
  const logsStore = reactor.use(LogsStore)

  return createActor((sig) => {
    const nodeId = sig.readAndTrack(NodeIdSignal)

    loggingContext.output = (logEvent) => {
      if (logEvent.type === "clear") {
        logsStore.clear()
        return
      }

      logsStore.addLogLine({ ...logEvent, node: nodeId })
    }
  })
}
