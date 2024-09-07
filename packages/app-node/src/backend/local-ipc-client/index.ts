import superjson from "superjson"

import { createConnection } from "node:net"

import type { ScopeContext } from "@dassie/lib-reactive"
import { createClient, createNodejsSocketLink } from "@dassie/lib-rpc/client"

import type { AppRouter } from "../rpc-server/app-router"

export const connectIpcClient = (context: ScopeContext) => {
  const ipcSocket = createConnection(
    process.env["DASSIE_IPC_SOCKET_PATH"] ?? "/run/dassie.sock",
  )

  const rpcClient = createClient<AppRouter>({
    connection: createNodejsSocketLink(ipcSocket),
    transformer: superjson,
  })

  context.scope.onCleanup(() => {
    ipcSocket.end()
  })

  return rpcClient.rpc
}
