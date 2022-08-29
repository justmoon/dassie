import { respondJson } from "@dassie/lib-http-server"
import type { EffectContext } from "@dassie/lib-reactive"

import { routerService } from "../http-server/serve-http"

export const registerQueryHttpHandler = (sig: EffectContext) => {
  const router = sig.get(routerService)

  router.get("/query", (_request, response) => {
    response.setHeader("Access-Control-Allow-Origin", "*")
    respondJson(response, 200, { nodes: [] })
  })
}
