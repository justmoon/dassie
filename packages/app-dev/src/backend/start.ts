import { bold, green } from "picocolors"

import { createActor, createReactor } from "@dassie/lib-reactive"

import { handleShutdownSignals } from "../common/effects/handle-shutdown-signals"
import { compileRunner } from "./effects/compile-runner"
import { handleFileChange } from "./effects/handle-file-change"
import { proxyByHostname } from "./effects/proxy-by-hostname"
import { regenerateNodeConfig } from "./effects/regenerate-node-config"
import { registerReactiveLogger } from "./effects/register-reactive-logger"
import { runNodes } from "./effects/run-nodes"
import { debugUiServer } from "./effects/serve-debug-ui"
import { listenForRpcWebSocket } from "./effects/serve-rpc"
import { viteNodeService } from "./services/vite-node-server"
import { viteService } from "./services/vite-server"

const rootActor = () =>
  createActor(async (sig) => {
    console.log(bold(`  Dassie${green("//dev")}\n`))

    sig.run(registerReactiveLogger)
    sig.run(proxyByHostname)
    await sig.run(listenForRpcWebSocket).result

    await sig.run(viteService, undefined, { register: true }).result
    sig.run(viteNodeService, undefined, { register: true })

    await sig.run(compileRunner).result
    sig.run(handleFileChange)
    await sig.run(runNodes).result
    await sig.run(debugUiServer).result

    sig.run(regenerateNodeConfig)

    sig.run(handleShutdownSignals)
  })

const start = () => createReactor(rootActor)

export default start
