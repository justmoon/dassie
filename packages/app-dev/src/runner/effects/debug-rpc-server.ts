import { initTRPC } from "@trpc/server"
import { applyWSSHandler } from "@trpc/server/adapters/ws"
import { observable } from "@trpc/server/observable"
import superjson from "superjson"
import { WebSocketServer } from "ws"

import { nodeTableStore } from "@dassie/app-node"
import { peerBalanceMapStore } from "@dassie/app-node/src/backend/balances/stores/peer-balance-map"
import { routingTableStore } from "@dassie/app-node/src/backend/peer-protocol/stores/routing-table"
import { createLogger } from "@dassie/lib-logger"
import { createActor, debugFirehose } from "@dassie/lib-reactive"
import {
  ReactiveContext,
  createRemoteReactiveRouter,
} from "@dassie/lib-reactive-trpc/server"

import { prettyFormat } from "../../common/utils/pretty-format"

const logger = createLogger("das:dev:runner:debug-rpc-server")

export const exposedStores = {
  nodeTable: nodeTableStore,
  routingTable: routingTableStore,
  balanceTable: peerBalanceMapStore,
} as const

export type ExposedStoresMap = typeof exposedStores

export const trpc = initTRPC.context<ReactiveContext>().create({
  transformer: superjson,
})
export const debugRpcRouter = trpc.mergeRouters(
  trpc.router({
    listenToFirehose: trpc.procedure.subscription(({ ctx }) => {
      return observable<{ topic: string; message: string }>((emit) => {
        return ctx.reactor.use(debugFirehose).on((event) => {
          emit.next({
            topic: event.topic.name,
            message: prettyFormat(event.message),
          })
        })
      })
    }),
  }),
  createRemoteReactiveRouter(trpc, exposedStores)
)

export type DebugRpcRouter = typeof debugRpcRouter

export const runDebugRpcServer = () =>
  createActor((sig) => {
    const port = Number(process.env["DASSIE_DEBUG_RPC_PORT"])

    if (!port) return

    logger.debug("starting debug rpc server", { port })

    const wss = new WebSocketServer({
      port,
    })
    const handler = applyWSSHandler({
      wss,
      router: debugRpcRouter,
      createContext: () => ({
        reactor: sig.reactor,
      }),
    })

    sig.onCleanup(() => {
      handler.broadcastReconnectNotification()
      wss.close()
    })
  })
