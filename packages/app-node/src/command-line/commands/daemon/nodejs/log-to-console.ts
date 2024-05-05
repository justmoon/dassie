import { createCliFormatter } from "@dassie/lib-logger"
import { type Reactor, createActor } from "@dassie/lib-reactive"

import { AttachLogger } from "../../../../backend"
import { LogsStore } from "../../../../common/stores/logs"

export const LogToConsoleActor = (reactor: Reactor) => {
  const logsStore = reactor.use(LogsStore)
  const attachLogger = reactor.use(AttachLogger)

  const format = createCliFormatter()

  return createActor((sig) => {
    attachLogger()

    logsStore.changes.on(sig, ([action, parameters]) => {
      if (action === "addLogLine") {
        process.stdout.write(format(parameters[0]) + "\n")
      }
    })
  })
}
