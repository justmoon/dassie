import { EffectContext, createReactor } from "@dassie/lib-reactive"

import { registerQueryHttpHandler } from "./data-exchange/register-query-http-handler"
import { registerRootRoute, serveHttp } from "./http-server/serve-http"
import { attachLogger } from "./logger"

export const rootEffect = (sig: EffectContext) => {
  sig.run(attachLogger)
  sig.run(serveHttp)
  sig.run(registerRootRoute)
  sig.run(registerQueryHttpHandler)
}

export const start = () => createReactor(rootEffect)
