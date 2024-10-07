import { createActor, createReactor } from "@dassie/lib-reactive"

import { ApplyDebugLoggingScopes } from "./actors/apply-debug-logging-scopes"
import { HandleFileChangeActor } from "./actors/handle-file-change"
import { HandleShutdownSignalsActor } from "./actors/handle-shutdown-signals"
import { PersistScenarioActor } from "./actors/persist-scenario"
import { ProxyByHostnameActor } from "./actors/proxy-by-hostname"
import { RegisterReactiveLoggerActor } from "./actors/register-reactive-logger"
import { RunScenarioActor } from "./actors/run-scenario"
import { DebugUiServerActor } from "./actors/serve-gui"
import { ServeRunnerRpcActor } from "./actors/serve-runner-rpc"
import { ServeUiRpcActor } from "./actors/serve-ui-rpc"
import { verifyPrerequisites } from "./functions/verify-prerequisites"
import type {
  DevelopmentActorContext,
  DevelopmentBase,
} from "./types/development-base"

export const RootActor = () =>
  createActor(async (sig: DevelopmentActorContext) => {
    sig.run(RegisterReactiveLoggerActor)
    sig.run(ApplyDebugLoggingScopes)
    sig.run(HandleShutdownSignalsActor)

    const isValidEnvironment = await verifyPrerequisites()

    if (!isValidEnvironment) {
      process.exitCode = 1
      void sig.reactor.dispose()
      return
    }

    sig.run(PersistScenarioActor)

    await sig.run(ServeUiRpcActor)
    sig.run(ServeRunnerRpcActor)
    await sig.run(DebugUiServerActor)

    sig.run(ProxyByHostnameActor)

    sig.run(HandleFileChangeActor)
    await sig.run(RunScenarioActor)

  })

export const prepareReactor = (parameters: DevelopmentBase) =>
  createReactor().withBase(parameters)
const start = async (parameters: DevelopmentBase) => {
  const reactor = prepareReactor(parameters)
  const startActor = reactor.use(RootActor)
  await startActor.run(reactor)
  return reactor
}

export default start
