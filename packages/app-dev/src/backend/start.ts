import { ViteDevServer } from "vite"
import { ViteNodeServer as ViteNodeServerType } from "vite-node/server"

import { createActor, createReactor } from "@dassie/lib-reactive"

import { HandleShutdownSignalsActor } from "../common/actors/handle-shutdown-signals"
import { CloseViteServerOnShutdownActor } from "./actors/close-vite-server-on-shutdown"
import { HandleFileChangeActor } from "./actors/handle-file-change"
import { ProxyByHostnameActor } from "./actors/proxy-by-hostname"
import { RegisterReactiveLoggerActor } from "./actors/register-reactive-logger"
import { RunNodesActor } from "./actors/run-nodes"
import { DebugUiServerActor } from "./actors/serve-debug-ui"
import { ListenForRpcWebSocketActor } from "./actors/serve-rpc"
import { RestartCallbackUnconstructable } from "./unconstructables/restart-callback"
import { ViteNodeServer } from "./unconstructables/vite-node-server"
import { ViteServer } from "./unconstructables/vite-server"

export interface StartParameters {
  viteServer: ViteDevServer
  viteNodeServer: ViteNodeServerType
  restart: () => void
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
    sig.run(CloseViteServerOnShutdownActor)
  })

const start = async ({
  viteServer,
  viteNodeServer,
  restart,
}: StartParameters) => {
  const reactor = createReactor()
  reactor.set(ViteServer, viteServer)
  reactor.set(ViteNodeServer, viteNodeServer)
  reactor.set(RestartCallbackUnconstructable, restart)
  const startActor = reactor.use(RootActor)
  await startActor.run(reactor)
  return reactor
}

export default start
