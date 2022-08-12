import { rootEffect as nodeRootEffect } from "@dassie/app-node"
import { createLogger } from "@dassie/lib-logger"
import { EffectContext, createReactor } from "@dassie/lib-reactive"

import { runDebugRpcServer } from "../effects/debug-rpc-server"
import { forwardLogs } from "../effects/forward-logs"
import { forwardPeerTraffic } from "../effects/forward-peer-traffic"
import { handleSigterm } from "../effects/handle-sigterm"

export const logger = createLogger("das:dev:launcher:node")

const rootEffect = async (sig: EffectContext) => {
  sig.use(handleSigterm)
  sig.use(forwardLogs)
  sig.use(forwardPeerTraffic)
  await sig.use(nodeRootEffect)
  sig.use(runDebugRpcServer)
}

createReactor(rootEffect)
