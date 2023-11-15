import { ViteDevServer } from "vite"
import { ViteNodeServer as ViteNodeServerType } from "vite-node/server"

import { createActor, createReactor } from "@dassie/lib-reactive"

import { HandleShutdownSignalsActor } from "../common/actors/handle-shutdown-signals"
import { HandleFileChangeActor } from "./actors/handle-file-change"
import { ProxyByHostnameActor } from "./actors/proxy-by-hostname"
import { RegisterReactiveLoggerActor } from "./actors/register-reactive-logger"
import { RunNodesActor } from "./actors/run-nodes"
import { DebugUiServerActor } from "./actors/serve-debug-ui"
import { ListenForRpcWebSocketActor } from "./actors/serve-rpc"
import { ViteNodeServer } from "./unconstructables/vite-node-server"
import { ViteServer } from "./unconstructables/vite-server"

export interface StartParameters {
  viteServer: ViteDevServer
  viteNodeServer: ViteNodeServerType
}

const RootActor = () =>
  createActor(async (sig) => {
    sig.run(RegisterReactiveLoggerActor)
    await sig.run(ListenForRpcWebSocketActor)
    await sig.run(DebugUiServerActor)

    sig.run(ProxyByHostnameActor)

    sig.run(HandleFileChangeActor)
    await sig.run(RunNodesActor)

    sig.run(HandleShutdownSignalsActor)
  })

const start = async ({ viteServer, viteNodeServer }: StartParameters) => {
  const reactor = createReactor()
  reactor.lifecycle.onCleanup(async () => {
    await viteServer.close()
  })
  reactor.set(ViteServer, viteServer)
  reactor.set(ViteNodeServer, viteNodeServer)
  const startActor = reactor.use(RootActor)
  await startActor.run(reactor, reactor.lifecycle)
  return reactor
}

export default start
