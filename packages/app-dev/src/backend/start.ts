import { bold, green } from "picocolors"

import { EffectContext, createReactor } from "@xen-ilp/lib-reactive"

import { captureLogs } from "./effects/capture-logs"
import { debugUiServer } from "./effects/debug-ui-server"
import { registerReactiveLogger } from "./effects/register-reactive-logger"
import { runNodes } from "./effects/run-nodes"
import { listenForUiWebSocket } from "./effects/ui-rpc-server"
import { validateDevelopmentEnvironment } from "./effects/validate-development-environment"
import { walletServer } from "./effects/wallet-server"

const rootEffect = async (sig: EffectContext) => {
  console.log(bold(`  Xen${green("//dev")}`))
  const isEnvironmentValid = await sig.use(validateDevelopmentEnvironment)
  if (!isEnvironmentValid) {
    return
  }

  sig.use(captureLogs)
  sig.use(registerReactiveLogger)
  sig.use(listenForUiWebSocket)
  await sig.use(runNodes)
  await sig.use(walletServer)
  await sig.use(debugUiServer)

  process.on("SIGINT", () => void sig.reactor.dispose())

  sig.onCleanup(() => {
    process.off("SIGINT", () => void sig.reactor.dispose())
  })
}

const start = () => createReactor(rootEffect)

export default start
