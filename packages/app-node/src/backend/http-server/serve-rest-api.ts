import { createRestApi } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { routerService } from "./serve-http"

export const restApiService = () =>
  createActor((sig) => {
    const router = sig.get(routerService)

    if (!router) return

    return createRestApi(router)
  })

export const serveRestApi = () =>
  createActor((sig) => {
    sig.run(restApiService)
  })
