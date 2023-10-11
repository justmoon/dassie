import { createRestApi } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { HttpsRouterServiceActor } from "./serve-https"

export const RestApiServiceActor = () =>
  createActor((sig) => {
    const router = sig.get(HttpsRouterServiceActor)

    if (!router) return

    return createRestApi(router)
  })

export const ServeRestApiActor = () =>
  createActor((sig) => {
    sig.run(RestApiServiceActor)
  })
