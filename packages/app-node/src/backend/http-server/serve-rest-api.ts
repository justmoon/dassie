import { createRestApi } from "@dassie/lib-http-server"
import { EffectContext, createService } from "@dassie/lib-reactive"

import { routerService } from "./serve-http"

export const restApiService = () =>
  createService((sig) => {
    const router = sig.get(routerService)

    if (!router) return

    return createRestApi(router)
  })

export const serveRestApi = (sig: EffectContext) => {
  sig.run(sig.use(restApiService).effect)
}
