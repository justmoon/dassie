import { createActor } from "@dassie/lib-reactive"

import { RegisterStatisticsHttpHandlerActor } from "./register-statistics-http-handler"

export const PublicApiServerActor = () =>
  createActor((sig) => {
    sig.run(RegisterStatisticsHttpHandlerActor)
  })
