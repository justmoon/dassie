import superjson from "superjson"

import type { AppRouter } from "@dassie/app-dassie/src/rpc-server/app-router"
import { createRpcReact } from "@dassie/lib-rpc-react"

export const { rpc, RpcProvider, useWebSocketClient } =
  createRpcReact<AppRouter>()

export const clientOptions = {
  transformer: superjson,
}
