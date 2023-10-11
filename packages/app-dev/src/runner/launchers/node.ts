import { daemonActor } from "@dassie/app-node"
import { HasTlsSignal } from "@dassie/app-node/src/backend/config/computed/has-tls"
import { createActor, createReactor } from "@dassie/lib-reactive"

import { HandleShutdownSignalsActor } from "../../common/actors/handle-shutdown-signals"
import { ForwardLogsActor } from "../actors/forward-logs"
import { ForwardPeerTrafficActor } from "../actors/forward-peer-traffic"
import { HandleDisconnectActor } from "../actors/handle-disconnect"
import { PatchIlpLoggerActor } from "../actors/patch-ilp-logger"
import { ReportPeeringStateActor } from "../actors/report-peering-state"
import { ServeWalletActor } from "../actors/serve-wallet"
import { TrpcClientServiceActor } from "../services/trpc-client"

const DebugRunnerActor = () =>
  createActor(async (sig) => {
    sig.run(TrpcClientServiceActor)
    sig.run(HandleShutdownSignalsActor)
    sig.run(HandleDisconnectActor)
    sig.run(ForwardLogsActor)
    sig.run(PatchIlpLoggerActor)
    sig.run(ForwardPeerTrafficActor)
    await sig.run(ReportPeeringStateActor)

    const hasTls = sig.get(HasTlsSignal)
    if (hasTls) {
      await sig.run(ServeWalletActor)
    }
    await sig.run(daemonActor)
  })

const reactor = createReactor()
await reactor.use(DebugRunnerActor).run(reactor, reactor.lifecycle)
