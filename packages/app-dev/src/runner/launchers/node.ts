import { rootEffect as nodeRootEffect } from "@xen-ilp/app-node"
import { createLogger } from "@xen-ilp/lib-logger"
import { EffectContext, createReactor } from "@xen-ilp/lib-reactive"

import { runDebugRpcServer } from "../effects/debug-rpc-server"
import { forwardEvents } from "../effects/forward-events"
import { forwardLogs } from "../effects/forward-logs"
import { forwardPeerTraffic } from "../effects/forward-peer-traffic"

export const logger = createLogger("xen:dev:launcher:node")

const rootEffect = (sig: EffectContext) => {
  sig.use(forwardLogs)
  sig.use(forwardEvents)
  sig.use(forwardPeerTraffic)
  sig.use(nodeRootEffect)
  sig.use(runDebugRpcServer)
}

createReactor(rootEffect)
