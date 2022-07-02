import * as trpc from "@trpc/server"
import { applyWSSHandler } from "@trpc/server/adapters/ws"
import type { ViteNodeServer } from "vite-node/server"
import { WebSocketServer } from "ws"
import { z } from "zod"

export interface NodeRpcServerContext {
  nodeServer: ViteNodeServer
}

export const startupTime = Date.now()

export const nodeRpcRouter = trpc
  .router<NodeRpcServerContext>()
  .query("fetchModule", {
    input: z.string(),
    resolve({ ctx, input }) {
      return ctx.nodeServer.fetchModule(input)
    },
  })
  .query("resolveId", {
    input: z.tuple([z.string(), z.string().optional()]),
    resolve({ ctx, input }) {
      console.log("server resolveId", input)
      return ctx.nodeServer.resolveId(...input)
    },
  })

export type NodeRpcRouter = typeof nodeRpcRouter

export default class NodeRpcServer {
  readonly wss: WebSocketServer

  constructor(readonly context: NodeRpcServerContext) {
    this.wss = new WebSocketServer({ port: 10_002 })
    applyWSSHandler<NodeRpcRouter>({
      wss: this.wss,
      router: nodeRpcRouter,
      createContext() {
        return context
      },
    })
  }
}
