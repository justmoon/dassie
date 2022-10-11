import type { EffectContext } from "@dassie/lib-reactive"

import { registerTrpcHttpUpgrade } from "./trpc-server"

export const startTrpcServer = (sig: EffectContext) => {
  sig.run(registerTrpcHttpUpgrade)
}
