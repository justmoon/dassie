import { createActor } from "@dassie/lib-reactive"

import { RegisterNodeListMetadataHttpHandlerActor } from "./register-node-list-metadata-http-handler"
import { RegisterStatisticsHttpHandlerActor } from "./register-statistics-http-handler"

export const PublicApiServerActor = () =>
  createActor((sig) => {
    sig.run(RegisterStatisticsHttpHandlerActor)
    sig.run(RegisterNodeListMetadataHttpHandlerActor)
  })
