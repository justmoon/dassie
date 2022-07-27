import * as trpc from "@trpc/server"
import * as trpcExpress from "@trpc/server/adapters/express"
import cors from "cors"
import express from "express"
import prettyFormat from "pretty-format"
import superjson from "superjson"
import { z } from "zod"

import { peerTableStore } from "@xen-ilp/app-node/src/peering/stores/peer-table"
import { createLogger } from "@xen-ilp/lib-logger"
import type { EffectContext, Reactor } from "@xen-ilp/lib-reactive"

import { Uint8ArrayFormatPlugin } from "../../frontend/utils/uint8array-format-plugin"
import { messageCache } from "../services/message-cache"

const logger = createLogger("xen:dev:launcher:debug-rpc-server")

export const debugRpcRouter = trpc
  .router<Reactor>()
  .transformer(superjson)
  .query("getPeerTable", {
    resolve({ ctx: reactor }) {
      return reactor.useContext(peerTableStore).read()
    },
  })
  .query("getMessage", {
    input: z.tuple([z.number()]),
    resolve({ ctx: reactor, input: [messageId] }) {
      const cache = reactor.useContext(messageCache)
      return {
        message: prettyFormat(cache.get(messageId), {
          highlight: true,
          plugins: [Uint8ArrayFormatPlugin],
        }),
      }
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
}
