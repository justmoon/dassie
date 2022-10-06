import { rootEffect as beaconRootEffect } from "@dassie/app-beacon"
import { createLogger } from "@dassie/lib-logger"
import { EffectContext, createReactor } from "@dassie/lib-reactive"

import { handleShutdownSignals } from "../../common/effects/handle-shutdown-signals"
import { forwardLogs } from "../effects/forward-logs"
import { handleDisconnect } from "../effects/handle-disconnect"
import { trpcClientService } from "../services/trpc-client"

export const logger = createLogger("das:dev:launcher:node")

const rootEffect = (sig: EffectContext) => {
  sig.run(sig.use(trpcClientService).effect)
  sig.run(handleShutdownSignals)
  sig.run(handleDisconnect)
  sig.run(forwardLogs)
  sig.run(beaconRootEffect)
}

createReactor(rootEffect)
