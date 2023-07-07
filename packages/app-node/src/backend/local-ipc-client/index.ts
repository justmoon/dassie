import { createTRPCProxyClient } from "@trpc/client"

import { createConnection } from "node:net"

import { LifecycleScope } from "@dassie/lib-reactive"
import { socketLink } from "@dassie/lib-trpc-ipc/link"

import { LocalIpcRouter } from "../local-ipc-server/actors/serve-local-ipc"

export const connectIpcClient = (lifecycle: LifecycleScope) => {
  const ipcSocket = createConnection(
    process.env["DASSIE_IPC_SOCKET_PATH"] ?? "/run/dassie/ipc.sock"
  )

  const ipcClient = createTRPCProxyClient<LocalIpcRouter>({
    links: [
      socketLink({
        socket: ipcSocket,
      }),
    ],
  })

  lifecycle.onCleanup(() => {
    ipcSocket.end()
  })

  return ipcClient
}
