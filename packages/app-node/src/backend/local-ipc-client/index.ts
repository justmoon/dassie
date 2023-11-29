import { createTRPCProxyClient } from "@trpc/client"
import superjson from "superjson"

import { createConnection } from "node:net"

import { LifecycleContext } from "@dassie/lib-reactive"
import { socketLink } from "@dassie/lib-trpc-ipc/link"

import { AppRouter } from "../trpc-server/app-router"

export const connectIpcClient = (context: LifecycleContext) => {
  const ipcSocket = createConnection(
    process.env["DASSIE_IPC_SOCKET_PATH"] ?? "/run/dassie.sock",
  )

  const ipcClient = createTRPCProxyClient<AppRouter>({
    links: [
      socketLink({
        socket: ipcSocket,
      }),
    ],
    transformer: superjson,
  })

  context.lifecycle.onCleanup(() => {
    ipcSocket.end()
  })

  return ipcClient
}
