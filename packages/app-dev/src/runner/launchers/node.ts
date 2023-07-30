import { daemonActor } from "@dassie/app-node"
import { hasTlsComputed } from "@dassie/app-node/src/backend/config/computed/has-tls"
import { createActor, createReactor } from "@dassie/lib-reactive"

import { handleShutdownSignals } from "../../common/actors/handle-shutdown-signals"
import { forwardLogs } from "../actors/forward-logs"
import { forwardPeerTraffic } from "../actors/forward-peer-traffic"
import { handleDisconnect } from "../actors/handle-disconnect"
import { patchIlpLogger } from "../actors/patch-ilp-logger"
import { reportPeeringState } from "../actors/report-peering-state"
import { serveWallet } from "../actors/serve-wallet"
import { trpcClientService } from "../services/trpc-client"

const debugRunner = () =>
  createActor(async (sig) => {
    sig.run(trpcClientService)
    sig.run(handleShutdownSignals)
    sig.run(handleDisconnect)
    sig.run(forwardLogs)
    sig.run(patchIlpLogger)
    sig.run(forwardPeerTraffic)
    await sig.run(reportPeeringState)

    const hasTls = sig.get(hasTlsComputed)
    if (hasTls) {
      await sig.run(serveWallet)
    }
    await sig.run(daemonActor)
  })

const reactor = createReactor()
await reactor.use(debugRunner).run(reactor)
