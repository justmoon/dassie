import { bold, green } from "picocolors"

import { EffectContext, createReactor } from "@dassie/lib-reactive"

import { handleShutdownSignals } from "../common/effects/handle-shutdown-signals"
import { captureLogs } from "./effects/capture-logs"
import { compileRunner } from "./effects/compile-runner"
import { handleFileChange } from "./effects/handle-file-change"
import { registerReactiveLogger } from "./effects/register-reactive-logger"
import { runBeacons } from "./effects/run-beacons"
import { runNodes } from "./effects/run-nodes"
import { debugUiServer } from "./effects/serve-debug-ui"
import { listenForRpcWebSocket } from "./effects/serve-rpc"
import { serveWallet } from "./effects/serve-wallet"
import { indexLogs } from "./features/logs"

const rootEffect = async (sig: EffectContext) => {
  console.log(bold(`  Dassie${green("//dev")}\n`))

  await sig.run(compileRunner)
  sig.run(captureLogs)
  sig.run(indexLogs)
  sig.run(registerReactiveLogger)
  sig.run(listenForRpcWebSocket)
  await sig.run(handleFileChange)
  await sig.run(runBeacons)
  await sig.run(runNodes)
  await sig.run(serveWallet)
  await sig.run(debugUiServer)

  sig.run(handleShutdownSignals)
}

const start = () => createReactor(rootEffect)

export default start
