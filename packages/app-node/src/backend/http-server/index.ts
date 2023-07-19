import { createActor } from "@dassie/lib-reactive"

import { databaseConfigSignal } from "../config/database-config"
import { serveFrontend } from "./serve-frontend"
import { serveHttp } from "./serve-http"
import { serveHttps } from "./serve-https"
import { serveRestApi } from "./serve-rest-api"

export const startHttpServer = () =>
  createActor((sig) => {
    const { hasTls } = sig.getKeys(databaseConfigSignal, ["hasTls"])
    sig.run(serveHttp)

    if (hasTls) {
      sig.run(serveHttps)
      sig.run(serveRestApi)
      sig.run(serveFrontend)
    }
  })
