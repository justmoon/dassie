import { createActor } from "@dassie/lib-reactive"

import { registerStatisticsHttpHandler } from "./register-statistics-http-handler"

export const startStatisticsServer = () =>
  createActor((sig) => {
    sig.run(registerStatisticsHttpHandler)
  })
