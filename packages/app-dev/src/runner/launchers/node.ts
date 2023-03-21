import { rootActor as nodeRootActor } from "@dassie/app-node"
import { createLogger } from "@dassie/lib-logger"
import { createActor, createReactor } from "@dassie/lib-reactive"

import { handleShutdownSignals } from "../../common/effects/handle-shutdown-signals"
import { runDebugRpcServer } from "../effects/debug-rpc-server"
import { forwardLogs } from "../effects/forward-logs"
import { forwardPeerTraffic } from "../effects/forward-peer-traffic"
import { handleDisconnect } from "../effects/handle-disconnect"
import { reportPeeringState } from "../effects/report-peering-state"
import { serveWallet } from "../effects/serve-wallet"
import { trpcClientService } from "../services/trpc-client"

export const logger = createLogger("das:dev:launcher:node")

const debugRunner = () =>
  createActor(async (sig) => {
    sig.run(sig.use(trpcClientService).effect)
    sig.run(handleShutdownSignals)
    sig.run(handleDisconnect)
    sig.run(forwardLogs)
    sig.run(forwardPeerTraffic)
    await sig.run(reportPeeringState)
    await sig.run(serveWallet)
    await sig.run(nodeRootActor)
    sig.run(runDebugRpcServer)
  })

createReactor(debugRunner)
