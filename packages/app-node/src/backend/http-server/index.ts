import type { EffectContext } from "@dassie/lib-reactive"

import { serveHttp } from "./serve-http"

export const startHttpServer = (sig: EffectContext) => {
  sig.run(serveHttp)
}
