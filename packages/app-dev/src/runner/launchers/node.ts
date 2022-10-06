import { rootEffect as nodeRootEffect } from "@dassie/app-node"
import { createLogger } from "@dassie/lib-logger"
import { EffectContext, createReactor } from "@dassie/lib-reactive"

import { handleShutdownSignals } from "../../common/effects/handle-shutdown-signals"
import { runDebugRpcServer } from "../effects/debug-rpc-server"
import { forwardLogs } from "../effects/forward-logs"
import { forwardPeerTraffic } from "../effects/forward-peer-traffic"
import { handleDisconnect } from "../effects/handle-disconnect"
import { serveWallet } from "../effects/serve-wallet"
import { trpcClientService } from "../services/trpc-client"

export const logger = createLogger("das:dev:launcher:node")

const rootEffect = async (sig: EffectContext) => {
  sig.run(sig.use(trpcClientService).effect)
  sig.run(handleShutdownSignals)
  sig.run(handleDisconnect)
  sig.run(forwardLogs)
  sig.run(forwardPeerTraffic)
  await sig.run(serveWallet)
  await sig.run(nodeRootEffect)
  sig.run(runDebugRpcServer)
}

createReactor(rootEffect)
