import chalk from "chalk"

import { createActor, createReactor } from "@dassie/lib-reactive"

import { handleShutdownSignals } from "../common/actors/handle-shutdown-signals"
import { handleFileChange } from "./actors/handle-file-change"
import { proxyByHostname } from "./actors/proxy-by-hostname"
import { registerReactiveLogger } from "./actors/register-reactive-logger"
import { runNodes } from "./actors/run-nodes"
import { debugUiServer } from "./actors/serve-debug-ui"
import { listenForRpcWebSocket } from "./actors/serve-rpc"
import { viteNodeService } from "./services/vite-node-server"
import { viteService } from "./services/vite-server"
import { compileRunner } from "./utils/compile-runner"

const rootActor = () =>
  createActor(async (sig) => {
    console.log(chalk.bold(`  Dassie${chalk.green("//dev")}\n`))

    sig.run(registerReactiveLogger)
    sig.run(proxyByHostname)
    await sig.run(listenForRpcWebSocket)

    await sig.run(viteService)
    sig.run(viteNodeService)

    await compileRunner()
    sig.run(handleFileChange)
    await sig.run(runNodes)
    await sig.run(debugUiServer)

    sig.run(handleShutdownSignals)
  })

const start = () => createReactor(rootActor)

export default start
