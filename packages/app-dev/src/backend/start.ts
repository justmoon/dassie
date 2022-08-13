import { bold, green } from "picocolors"

import { EffectContext, createReactor } from "@dassie/lib-reactive"

import { captureLogs } from "./effects/capture-logs"
import { compileRunner } from "./effects/compile-runner"
import { registerReactiveLogger } from "./effects/register-reactive-logger"
import { runNodes } from "./effects/run-nodes"
import { debugUiServer } from "./effects/serve-debug-ui"
import { listenForRpcWebSocket } from "./effects/serve-rpc"
import { serveWallet } from "./effects/serve-wallet"
import { indexLogs } from "./features/logs"

const rootEffect = async (sig: EffectContext) => {
  console.log(bold(`  Dassie${green("//dev")}`))

  await sig.run(compileRunner)
  sig.run(captureLogs)
  sig.run(indexLogs)
  sig.run(registerReactiveLogger)
  sig.run(listenForRpcWebSocket)
  await sig.run(runNodes)
  await sig.run(serveWallet)
  await sig.run(debugUiServer)

  process.on("SIGINT", () => void sig.reactor.dispose())

  sig.onCleanup(() => {
    process.off("SIGINT", () => void sig.reactor.dispose())
  })
}

const start = () => createReactor(rootEffect)

export default start
