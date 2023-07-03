import { rootActor as nodeRootActor } from "@dassie/app-node"
import { databaseConfigPlain } from "@dassie/app-node/src/backend/config/database-config"
import { createLogger } from "@dassie/lib-logger"
import { createActor, createReactor } from "@dassie/lib-reactive"

import { handleShutdownSignals } from "../../common/actors/handle-shutdown-signals"
import { runDebugRpcServer } from "../actors/debug-rpc-server"
import { forwardLogs } from "../actors/forward-logs"
import { forwardPeerTraffic } from "../actors/forward-peer-traffic"
import { handleDisconnect } from "../actors/handle-disconnect"
import { patchIlpLogger } from "../actors/patch-ilp-logger"
import { reportPeeringState } from "../actors/report-peering-state"
import { serveWallet } from "../actors/serve-wallet"
import { trpcClientService } from "../services/trpc-client"

export const logger = createLogger("das:dev:launcher:node")

const debugRunner = () =>
  createActor(async (sig) => {
    const config = sig.use(databaseConfigPlain)
    sig.run(trpcClientService)
    sig.run(handleShutdownSignals)
    sig.run(handleDisconnect)
    sig.run(forwardLogs)
    sig.run(patchIlpLogger)
    sig.run(forwardPeerTraffic)
    await sig.run(reportPeeringState)
    if (config.hasWebUi) {
      await sig.run(serveWallet)
    }
    await sig.run(nodeRootActor)
    sig.run(runDebugRpcServer)
  })

const reactor = createReactor()
await reactor.use(debugRunner).run(reactor)
