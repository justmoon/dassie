import type { EffectContext } from "@dassie/lib-reactive"

import { serveHttp } from "./serve-http"
import { serveRestApi } from "./serve-rest-api"

export const startHttpServer = (sig: EffectContext) => {
  sig.run(serveHttp)
  sig.run(serveRestApi)
}
