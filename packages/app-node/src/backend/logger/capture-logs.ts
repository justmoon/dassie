import {
  compareLogLevel,
  createCliFormatter,
  context as loggingContext,
} from "@dassie/lib-logger"
import { type Reactor, createActor } from "@dassie/lib-reactive"

import { LogsStore } from "../../common/stores/logs"
import { HasNodeIdentitySignal } from "../config/computed/has-node-identity"
import { NodeIdSignal } from "../ilp-connector/computed/node-id"
import { StdoutLogLevel } from "./signals/stdout-log-level"

export const CaptureLogsActor = (reactor: Reactor) => {
  const logsStore = reactor.use(LogsStore)
  const stdoutLogLevel = reactor.use(StdoutLogLevel)

  const cliFormatter = createCliFormatter()

  return createActor((sig) => {
    const hasNodeIdentity = sig.readAndTrack(HasNodeIdentitySignal)
    const nodeId = hasNodeIdentity ? sig.readAndTrack(NodeIdSignal) : "none"

    loggingContext.output = (logEvent) => {
      if (logEvent.type === "clear") {
        logsStore.clear()
        return
      }

      logsStore.addLogLine({ ...logEvent, node: nodeId })

      const currentLogLevel = stdoutLogLevel.read()
      if (
        currentLogLevel !== "none" &&
        compareLogLevel(logEvent.type, currentLogLevel) >= 0
      ) {
        process.stdout.write(cliFormatter(logEvent) + "\n")
      }
    }
  })
}
