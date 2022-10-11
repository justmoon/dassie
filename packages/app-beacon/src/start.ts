import { EffectContext, createReactor } from "@dassie/lib-reactive"

import { startDataExchange } from "./data-exchange"
import { startHttpServer } from "./http-server"
import { attachLogger } from "./logger"

export const rootEffect = (sig: EffectContext) => {
  sig.run(attachLogger)
  sig.run(startDataExchange)
  sig.run(startHttpServer)
}

export const start = () => createReactor(rootEffect)
