import type { ViteDevServer } from "vite"
import { ViteNodeServer as ViteNodeServerType } from "vite-node/server"

import { createActor, createReactor } from "@dassie/lib-reactive"

import { HandleShutdownSignalsActor } from "../common/actors/handle-shutdown-signals"
import { ApplyDebugLoggingScopes } from "./actors/apply-debug-logging-scopes"
import { HandleFileChangeActor } from "./actors/handle-file-change"
import { ProxyByHostnameActor } from "./actors/proxy-by-hostname"
import { RegisterReactiveLoggerActor } from "./actors/register-reactive-logger"
import { RunScenarioActor } from "./actors/run-scenario"
import { DebugUiServerActor } from "./actors/serve-debug-ui"
import { ServeRunnerRpcActor } from "./actors/serve-runner-rpc"
import { ServeUiRpcActor } from "./actors/serve-ui-rpc"
import { verifyPrerequisites } from "./functions/verify-prerequisites"
import { ViteNodeServer } from "./unconstructables/vite-node-server"
import { ViteServer } from "./unconstructables/vite-server"

export interface StartParameters {
  viteServer: ViteDevServer
  viteNodeServer: ViteNodeServerType
}

export const RootActor = () =>
  createActor(async (sig) => {
    sig.run(RegisterReactiveLoggerActor)
    sig.run(ApplyDebugLoggingScopes)

    const isValidEnvironment = await verifyPrerequisites()

    if (!isValidEnvironment) {
      process.exitCode = 1
      void sig.reactor.dispose()
      return
    }

    await sig.run(ServeUiRpcActor)
    sig.run(ServeRunnerRpcActor)
    await sig.run(DebugUiServerActor)

    sig.run(ProxyByHostnameActor)

    sig.run(HandleFileChangeActor)
    await sig.run(RunScenarioActor)

    sig.run(HandleShutdownSignalsActor)
  })

export const prepareReactor = ({
  viteServer,
  viteNodeServer,
}: StartParameters) => {
  const reactor = createReactor()
  reactor.set(ViteServer, viteServer)
  reactor.set(ViteNodeServer, viteNodeServer)
  return reactor
}

const start = async (parameters: StartParameters) => {
  const reactor = prepareReactor(parameters)
  const startActor = reactor.use(RootActor)
  await startActor.run(reactor)
  return reactor
}

export default start
