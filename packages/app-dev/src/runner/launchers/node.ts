import { DaemonActor } from "@dassie/app-node"
import { HasTlsSignal } from "@dassie/app-node/src/backend/config/computed/has-tls"
import { createActor, createReactor } from "@dassie/lib-reactive"
import { createNodeRuntime } from "@dassie/lib-reactive-io/node"

import { HandleShutdownSignalsActor } from "../../common/actors/handle-shutdown-signals"
import { ForwardLogsActor } from "../actors/forward-logs"
import { ForwardPeerTrafficActor } from "../actors/forward-peer-traffic"
import { HandleDisconnectActor } from "../actors/handle-disconnect"
import { PatchIlpLoggerActor } from "../actors/patch-ilp-logger"
import { ReportPeeringStateActor } from "../actors/report-peering-state"
import { ServeHttpsActor } from "../actors/serve-https"
import { ServeWalletActor } from "../actors/serve-wallet"
import { RpcClientServiceActor } from "../services/rpc-client"

const DebugRunnerActor = () =>
  createActor(async (sig) => {
    const rpcClient = sig.run(RpcClientServiceActor)

    if (!rpcClient) {
      throw new Error("Failed to set up RPC client")
    }

    const rpcReactor = sig.withBase({ rpc: rpcClient.rpc })

    rpcReactor.run(ForwardLogsActor)
    sig.run(HandleShutdownSignalsActor)
    sig.run(HandleDisconnectActor)
    sig.run(PatchIlpLoggerActor)
    rpcReactor.run(ForwardPeerTrafficActor)
    await rpcReactor.run(ReportPeeringStateActor)

    await sig.withBase(createNodeRuntime()).run(DaemonActor)

    const hasTls = sig.readAndTrack(HasTlsSignal)
    if (hasTls) {
      await sig.run(ServeWalletActor)
      sig.run(ServeHttpsActor)
    }
  })

const reactor = createReactor()
await reactor.use(DebugRunnerActor).run(reactor)
