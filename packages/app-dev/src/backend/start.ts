import { bold, green } from "picocolors"

import { createActor, createReactor } from "@dassie/lib-reactive"

import { handleShutdownSignals } from "../common/actors/handle-shutdown-signals"
import { handleFileChange } from "./actors/handle-file-change"
import { proxyByHostname } from "./actors/proxy-by-hostname"
import { regenerateNodeConfig } from "./actors/regenerate-node-config"
import { registerReactiveLogger } from "./actors/register-reactive-logger"
import { runNodes } from "./actors/run-nodes"
import { debugUiServer } from "./actors/serve-debug-ui"
import { listenForRpcWebSocket } from "./actors/serve-rpc"
import { viteNodeService } from "./services/vite-node-server"
import { viteService } from "./services/vite-server"
import { compileRunner } from "./utils/compile-runner"

const rootActor = () =>
  createActor(async (sig) => {
    console.log(bold(`  Dassie${green("//dev")}\n`))

    sig.run(registerReactiveLogger)
    sig.run(proxyByHostname)
    await sig.run(listenForRpcWebSocket).result

    await sig.run(viteService, undefined, { register: true }).result
    sig.run(viteNodeService, undefined, { register: true })

    await compileRunner()
    sig.run(handleFileChange)
    await sig.run(runNodes).result
    await sig.run(debugUiServer).result

    sig.run(regenerateNodeConfig)

    sig.run(handleShutdownSignals)
  })

const start = () => createReactor(rootActor)

export default start
