import { bold, green } from "picocolors"

import { EffectContext, createReactor } from "@dassie/lib-reactive"

import { handleShutdownSignals } from "../common/effects/handle-shutdown-signals"
import { compileRunner } from "./effects/compile-runner"
import { handleFileChange } from "./effects/handle-file-change"
import { proxyByHostname } from "./effects/proxy-by-hostname"
import { registerReactiveLogger } from "./effects/register-reactive-logger"
import { runBeacons } from "./effects/run-beacons"
import { runNodes } from "./effects/run-nodes"
import { debugUiServer } from "./effects/serve-debug-ui"
import { listenForRpcWebSocket } from "./effects/serve-rpc"
import { viteNodeService } from "./services/vite-node-server"
import { viteService } from "./services/vite-server"

const rootEffect = async (sig: EffectContext) => {
  console.log(bold(`  Dassie${green("//dev")}\n`))

  sig.run(registerReactiveLogger)
  await sig.run(listenForRpcWebSocket)

  sig.run(sig.use(viteService).effect)
  sig.run(sig.use(viteNodeService).effect)

  await sig.run(compileRunner)
  await sig.run(handleFileChange)
  await sig.run(runBeacons)
  await sig.run(runNodes)
  await sig.run(debugUiServer)
  sig.run(proxyByHostname)

  sig.run(handleShutdownSignals)
}

const start = () => createReactor(rootEffect)

export default start
