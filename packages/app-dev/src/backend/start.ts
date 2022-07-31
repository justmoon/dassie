import { bold, green } from "picocolors"

import { EffectContext, createReactor } from "@xen-ilp/lib-reactive"

import { captureLogs } from "./effects/capture-logs"
import { compileRunner } from "./effects/compile-runner"
import { registerReactiveLogger } from "./effects/register-reactive-logger"
import { runNodes } from "./effects/run-nodes"
import { debugUiServer } from "./effects/serve-debug-ui"
import { listenForRpcWebSocket } from "./effects/serve-rpc"
import { serveWallet } from "./effects/serve-wallet"
import { validateDevelopmentEnvironment } from "./effects/validate-development-environment"
import { indexLogs } from "./features/logs"

const rootEffect = async (sig: EffectContext) => {
  console.log(bold(`  Xen${green("//dev")}`))
  const isEnvironmentValid = await sig.use(validateDevelopmentEnvironment)
  if (!isEnvironmentValid) {
    return
  }

  await sig.use(compileRunner)
  sig.use(captureLogs)
  sig.use(indexLogs)
  sig.use(registerReactiveLogger)
  sig.use(listenForRpcWebSocket)
  await sig.use(runNodes)
  await sig.use(serveWallet)
  await sig.use(debugUiServer)

  process.on("SIGINT", () => void sig.reactor.dispose())

  sig.onCleanup(() => {
    process.off("SIGINT", () => void sig.reactor.dispose())
  })
}

const start = () => createReactor(rootEffect)

export default start
