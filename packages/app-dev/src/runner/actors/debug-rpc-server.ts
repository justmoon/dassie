import { initTRPC } from "@trpc/server"
import { applyWSSHandler } from "@trpc/server/adapters/ws"
import { observable } from "@trpc/server/observable"
import superjson from "superjson"
import { WebSocketServer } from "ws"

import { createLogger } from "@dassie/lib-logger"
import {
  InferMessageType,
  createActor,
  debugFirehose,
} from "@dassie/lib-reactive"
import { type ReactiveContext } from "@dassie/lib-reactive-trpc/server"

import { prettyFormat } from "../../common/utils/pretty-format"

const logger = createLogger("das:dev:runner:debug-rpc-server")

export const trpc = initTRPC.context<ReactiveContext>().create({
  transformer: superjson,
})
export const debugRpcRouter = trpc.router({
  listenToFirehose: trpc.procedure.subscription(({ ctx }) => {
    return observable<{ topic: string; message: string }>((emit) => {
      const firehose = ctx.reactor.use(debugFirehose)
      const listener = (event: InferMessageType<typeof debugFirehose>) => {
        emit.next({
          topic: event.topic.name,
          message: prettyFormat(event.message),
        })
      }
      firehose.on(ctx.reactor, listener)
      return () => firehose.off(listener)
    })
  }),
})

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
