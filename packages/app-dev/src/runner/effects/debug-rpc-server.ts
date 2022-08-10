import { nodeTableStore, peerTableStore } from "@dassie/app-node"
import { createLogger } from "@dassie/lib-logger"
import { EffectContext, Reactor, debugFirehose } from "@dassie/lib-reactive"
import { createRemoteReactiveRouter } from "@dassie/lib-reactive-trpc/server"
import * as trpc from "@trpc/server"
import { applyWSSHandler } from "@trpc/server/adapters/ws"
import prettyFormat from "pretty-format"
import superjson from "superjson"
import { WebSocketServer } from "ws"

const logger = createLogger("das:dev:launcher:debug-rpc-server")

export const exposedStores = {
  peerTable: peerTableStore,
  nodeTable: nodeTableStore,
} as const

export type ExposedStoresMap = typeof exposedStores

export const debugRpcRouter = trpc
  .router<Reactor>()
  .transformer(superjson)
  .merge(createRemoteReactiveRouter(exposedStores))
  .subscription("listenToFirehose", {
    resolve({ ctx: { useContext: fromContext } }) {
      return new trpc.Subscription<{ topic: string; message: string }>(
        (sendToClient) => {
          return fromContext(debugFirehose).on((event) => {
            sendToClient.data({
              topic: event.topic.name,
              message: prettyFormat(event.message),
            })
          })
        }
      )
    },
  })

export type DebugRpcRouter = typeof debugRpcRouter

export const runDebugRpcServer = (sig: EffectContext) => {
  const port = Number(process.env["DASSIE_DEBUG_RPC_PORT"])

  if (!port) return

  logger.debug("starting debug rpc server", { port })

  const wss = new WebSocketServer({
    port,
  })
  const handler = applyWSSHandler({
    wss,
    router: debugRpcRouter,
    createContext: () => sig.reactor,
  })

  wss.on("connection", (ws) => {
    logger.info(`➕➕ Connection (${wss.clients.size})`)
    ws.once("close", () => {
      logger.info(`➖➖ Connection (${wss.clients.size})`)
    })
  })
  logger.info("✅ WebSocket Server listening on ws://localhost:3001")

  sig.onCleanup(() => {
    handler.broadcastReconnectNotification()
    wss.close()
  })
}
