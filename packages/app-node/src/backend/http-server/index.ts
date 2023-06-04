import { createActor } from "@dassie/lib-reactive"

import { serveFrontend } from "./serve-frontend"
import { serveHttp } from "./serve-http"
import { serveRestApi } from "./serve-rest-api"

export const startHttpServer = () =>
  createActor((sig) => {
    sig.run(serveHttp)
    sig.run(serveRestApi)
    sig.run(serveFrontend)
  })
