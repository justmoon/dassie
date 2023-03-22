import { createActor, createReactor } from "@dassie/lib-reactive"

import { startDataExchange } from "./data-exchange"
import { startHttpServer } from "./http-server"
import { attachLogger } from "./logger"

export const rootActor = () =>
  createActor((sig) => {
    sig.run(attachLogger)
    sig.run(startHttpServer)
    sig.run(startDataExchange)
  })

export const start = () => createReactor(rootActor)
