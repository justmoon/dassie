import * as trpc from "@trpc/server"
import * as trpcExpress from "@trpc/server/adapters/express"
import cors from "cors"
import express from "express"
import superjson from "superjson"

import { peerTableStore } from "@xen-ilp/app-node/src/peering/stores/peer-table"
import { createLogger } from "@xen-ilp/lib-logger"
import type { EffectContext, Reactor } from "@xen-ilp/lib-reactive"

const logger = createLogger("xen:dev:launcher:debug-rpc-server")

export const debugRpcRouter = trpc
  .router<Reactor>()
  .transformer(superjson)
  .query("getPeerTable", {
    resolve({ ctx: reactor }) {
      return reactor.fromContext(peerTableStore).read()
    },
  })

export type DebugRpcRouter = typeof debugRpcRouter

export const runDebugRpcServer = (sig: EffectContext) => {
  const port = Number(process.env["XEN_DEBUG_RPC_PORT"])

  if (!port) return

  logger.debug("starting debug rpc server", { port })

  const app = express()

  app.use(cors())

  app.use(
    "/trpc",
    trpcExpress.createExpressMiddleware({
      router: debugRpcRouter,
      createContext: () => sig.reactor,
    })
  )

  app.listen(port)
  // const wss = new WebSocketServer({ port })
  // applyWSSHandler<DebugRpcRouter>({
  //   wss,
  //   router: debugRpcRouter,
  //   createContext: () => sig.reactor,
  // })
  // sig.onCleanup(() => {
  //   wss.close()
  // })
}
