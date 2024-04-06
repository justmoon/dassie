import superjson from "superjson"

import { createConnection } from "node:net"

import { LifecycleContext } from "@dassie/lib-reactive"
import { createClient, createNodejsSocketLink } from "@dassie/lib-rpc/client"

import { AppRouter } from "../rpc-server/app-router"

export const connectIpcClient = (context: LifecycleContext) => {
  const ipcSocket = createConnection(
    process.env["DASSIE_IPC_SOCKET_PATH"] ?? "/run/dassie.sock",
  )

  const rpcClient = createClient<AppRouter>({
    connection: createNodejsSocketLink(ipcSocket),
    transformer: superjson,
  })

  context.lifecycle.onCleanup(() => {
    ipcSocket.end()
  })

  return rpcClient.rpc
}
