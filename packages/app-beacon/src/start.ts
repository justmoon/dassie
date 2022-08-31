import { EffectContext, createReactor } from "@dassie/lib-reactive"

import { registerPingHttpHandler } from "./data-exchange/register-ping-http-handler"
import { registerQueryHttpHandler } from "./data-exchange/register-query-http-handler"
import { trackNodes } from "./data-exchange/track-nodes"
import { registerRootRoute, serveHttp } from "./http-server/serve-http"
import { attachLogger } from "./logger"

export const rootEffect = (sig: EffectContext) => {
  sig.run(attachLogger)
  sig.run(serveHttp)
  sig.run(registerRootRoute)
  sig.run(registerPingHttpHandler)
  sig.run(registerQueryHttpHandler)
  sig.run(trackNodes)
}

export const start = () => createReactor(rootEffect)
