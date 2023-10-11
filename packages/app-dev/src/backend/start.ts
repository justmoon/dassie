import { ViteDevServer } from "vite"
import { ViteNodeServer } from "vite-node/server"

import { createActor, createReactor } from "@dassie/lib-reactive"

import { HandleShutdownSignalsActor } from "../common/actors/handle-shutdown-signals"
import { HandleFileChangeActor } from "./actors/handle-file-change"
import { ProxyByHostnameActor } from "./actors/proxy-by-hostname"
import { RegisterReactiveLoggerActor } from "./actors/register-reactive-logger"
import { RunNodesActor } from "./actors/run-nodes"
import { DebugUiServerActor } from "./actors/serve-debug-ui"
import { ListenForRpcWebSocketActor } from "./actors/serve-rpc"

export interface StartParameters {
  viteServer: ViteDevServer
  viteNodeServer: ViteNodeServer
}

const RootActor = () =>
  createActor(async (sig, { viteServer, viteNodeServer }: StartParameters) => {
    sig.run(RegisterReactiveLoggerActor)
    await sig.run(ListenForRpcWebSocketActor)
    await sig.run(DebugUiServerActor)

    sig.run(ProxyByHostnameActor)

    sig.run(HandleFileChangeActor, { viteServer, viteNodeServer })
    await sig.run(RunNodesActor, { viteServer, viteNodeServer })

    sig.run(HandleShutdownSignalsActor)
  })

const start = async ({ viteServer, viteNodeServer }: StartParameters) => {
  const reactor = createReactor()
  reactor.lifecycle.onCleanup(async () => {
    await viteServer.close()
  })
  const startActor = reactor.use(RootActor)
  await startActor.run(reactor, reactor.lifecycle, {
    viteServer,
    viteNodeServer,
  })
  return reactor
}

export default start
