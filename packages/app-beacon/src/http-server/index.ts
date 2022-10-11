import type { EffectContext } from "@dassie/lib-reactive"

import { registerRootRoute, serveHttp } from "./serve-http"

export const startHttpServer = (sig: EffectContext) => {
  sig.run(serveHttp)
  sig.run(registerRootRoute)
}
