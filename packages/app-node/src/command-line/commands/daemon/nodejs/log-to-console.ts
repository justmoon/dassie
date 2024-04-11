import { createActor } from "@dassie/lib-reactive"

import { LogsStore } from "../../../../common/stores/logs"

export const LogToConsoleActor = (reactor: Reactor) => {
  return createActor(() => {
    const logsStore = reactor.use(LogsStore)
  })
}
