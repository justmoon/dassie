import { createRestApi } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { httpsRouterService } from "./serve-https"

export const restApiService = () =>
  createActor((sig) => {
    const router = sig.get(httpsRouterService)

    if (!router) return

    return createRestApi(router)
  })

export const serveRestApi = () =>
  createActor((sig) => {
    sig.run(restApiService)
  })
