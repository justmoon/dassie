import type { EffectContext } from "@dassie/lib-reactive"

import { registerPingHttpHandler } from "./register-ping-http-handler"
import { registerQueryHttpHandler } from "./register-query-http-handler"
import { trackNodes } from "./track-nodes"

export const startDataExchange = (sig: EffectContext) => {
  sig.run(registerPingHttpHandler)
  sig.run(registerQueryHttpHandler)
  sig.run(trackNodes)
}
