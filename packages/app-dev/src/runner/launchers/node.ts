import { rootActor as nodeRootActor } from "@dassie/app-node"
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
    sig.run(trpcClientService, undefined, { register: true })
    sig.run(handleShutdownSignals)
    sig.run(handleDisconnect)
    sig.run(forwardLogs)
    sig.run(patchIlpLogger)
    sig.run(forwardPeerTraffic)
    await sig.run(reportPeeringState).result
    await sig.run(serveWallet).result
    await sig.run(nodeRootActor).result
    sig.run(runDebugRpcServer)
  })

createReactor(debugRunner)
