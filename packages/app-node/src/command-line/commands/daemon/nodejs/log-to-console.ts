import { type Reactor, createActor } from "@dassie/lib-reactive"

import { AttachLogger } from "../../../../backend"

export const LogToConsoleActor = (reactor: Reactor) => {
  const attachLogger = reactor.use(AttachLogger)

  return createActor(() => {
    attachLogger()
  })
}
