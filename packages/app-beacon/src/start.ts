import { EffectContext, createReactor } from "@dassie/lib-reactive"

import { registerRootRoute, serveHttp } from "./http-server/serve-http"
import { attachLogger } from "./logger"

export const rootEffect = (sig: EffectContext) => {
  sig.run(attachLogger)
  sig.run(serveHttp)
  sig.run(registerRootRoute)
}

export const start = () => createReactor(rootEffect)
