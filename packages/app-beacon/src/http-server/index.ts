import { createActor } from "@dassie/lib-reactive"

import { serveHttp } from "./serve-http"

export const startHttpServer = () =>
  createActor((sig) => {
    sig.run(serveHttp)
  })
