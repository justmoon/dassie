import chalk from "chalk"
import { ViteDevServer } from "vite"
import { ViteNodeServer } from "vite-node/server"

import { createActor, createReactor } from "@dassie/lib-reactive"

import { handleShutdownSignals } from "../common/actors/handle-shutdown-signals"
import { handleFileChange } from "./actors/handle-file-change"
import { proxyByHostname } from "./actors/proxy-by-hostname"
import { registerReactiveLogger } from "./actors/register-reactive-logger"
import { runNodes } from "./actors/run-nodes"
import { debugUiServer } from "./actors/serve-debug-ui"
import { listenForRpcWebSocket } from "./actors/serve-rpc"
import { compileRunner } from "./utils/compile-runner"

export interface StartParameters {
  viteServer: ViteDevServer
  viteNodeServer: ViteNodeServer
}

const rootActor = () =>
  createActor(async (sig, { viteServer, viteNodeServer }: StartParameters) => {
    // eslint-disable-next-line no-console
    console.log(chalk.bold(`  Dassie${chalk.green("//dev")}\n`))

    sig.run(registerReactiveLogger)
    await sig.run(listenForRpcWebSocket)
    await sig.run(debugUiServer)

    sig.run(proxyByHostname)

    await compileRunner()
    sig.run(handleFileChange, { viteServer, viteNodeServer })
    await sig.run(runNodes, { viteServer, viteNodeServer })

    sig.run(handleShutdownSignals)
  })

const start = async ({ viteServer, viteNodeServer }: StartParameters) => {
  const reactor = createReactor()
  reactor.onCleanup(async () => {
    await viteServer.close()
  })
  const startActor = reactor.use(rootActor)
  await startActor.run(reactor, { viteServer, viteNodeServer })
  return reactor
}

export default start
