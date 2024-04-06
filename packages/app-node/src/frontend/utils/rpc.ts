import superjson from "superjson"

import { createRpcReact } from "@dassie/lib-rpc-react"

import type { AppRouter } from "../../backend/rpc-server/app-router"

export const { rpc, RpcProvider, useWebSocketClient } =
  createRpcReact<AppRouter>()

export const clientOptions = {
  transformer: superjson,
}
