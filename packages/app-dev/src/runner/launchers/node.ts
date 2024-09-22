import { AttachLogger, DaemonActor } from "@dassie/app-dassie"
import { createActor, createReactor } from "@dassie/lib-reactive"
import { createRuntime } from "@dassie/lib-reactive-io/node"

import { HandleShutdownSignalsActor } from "../../actors/handle-shutdown-signals"
import { ForwardLogsActor } from "../actors/forward-logs"
import { ForwardPeerTrafficActor } from "../actors/forward-peer-traffic"
import { HandleDisconnectActor } from "../actors/handle-disconnect"
import { PatchIlpLoggerActor } from "../actors/patch-ilp-logger"
import { ReportPeeringStateActor } from "../actors/report-peering-state"
import { ServeHttpsActor } from "../actors/serve-https"
import { RpcClientServiceActor } from "../services/rpc-client"

const DebugRunnerActor = () =>
  createActor(async (sig) => {
    const attachLogger = sig.reactor.use(AttachLogger)
    attachLogger()

    const rpcClient = sig.run(RpcClientServiceActor)

    if (!rpcClient) {
      throw new Error("Failed to set up RPC client")
    }

    const rpcContext = sig.withBase({ rpc: rpcClient.rpc })

    rpcContext.run(ForwardLogsActor)
    sig.run(HandleShutdownSignalsActor)
    sig.run(HandleDisconnectActor)
    sig.run(PatchIlpLoggerActor)
    rpcContext.run(ForwardPeerTrafficActor)
    await rpcContext.run(ReportPeeringStateActor)

    const dassieContext = sig.withBase(createRuntime())

    await dassieContext.run(DaemonActor)
    await dassieContext.run(ServeHttpsActor)
  })

const reactor = createReactor()
await reactor.use(DebugRunnerActor).run(reactor)
