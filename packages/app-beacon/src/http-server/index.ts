import { createActor } from "@dassie/lib-reactive"

import { registerRootRoute, serveHttp } from "./serve-http"

export const startHttpServer = () =>
  createActor((sig) => {
    sig.run(serveHttp)
    sig.run(registerRootRoute)
  })
