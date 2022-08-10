import { rootEffect as nodeRootEffect } from "@xen-ilp/app-node"
import { createLogger } from "@xen-ilp/lib-logger"
import { EffectContext, createReactor } from "@xen-ilp/lib-reactive"

import { runDebugRpcServer } from "../effects/debug-rpc-server"
import { forwardLogs } from "../effects/forward-logs"
import { forwardPeerTraffic } from "../effects/forward-peer-traffic"
import { handleSigterm } from "../effects/handle-sigterm"

export const logger = createLogger("xen:dev:launcher:node")

const rootEffect = (sig: EffectContext) => {
  sig.use(handleSigterm)
  sig.use(forwardLogs)
  sig.use(forwardPeerTraffic)
  sig.use(nodeRootEffect)
  sig.use(runDebugRpcServer)
}

createReactor(rootEffect)
