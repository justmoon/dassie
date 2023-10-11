import { createActor } from "@dassie/lib-reactive"

import { RegisterStatisticsHttpHandlerActor } from "./register-statistics-http-handler"

export const StatisticsServerActor = () =>
  createActor((sig) => {
    sig.run(RegisterStatisticsHttpHandlerActor)
  })
