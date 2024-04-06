import { AxiosError } from "axios"
import superjson, { allowErrorProps, registerClass } from "superjson"
import { WebSocket } from "ws"

import { createActor } from "@dassie/lib-reactive"
import { createClient, createWebSocketLink } from "@dassie/lib-rpc/client"

import type { AppRouter } from "../../backend/rpc-routers/app-router"

export const RpcClientServiceActor = () =>
  createActor((sig) => {
    allowErrorProps("stack", "cause")
    registerClass(AggregateError, {
      allowProps: ["errors", "message", "stack", "cause"],
    })
    registerClass(AxiosError, {
      allowProps: ["code", "errors", "message", "name", "config", "cause"],
    })

    const rpcClient = createClient<AppRouter>({
      connection: createWebSocketLink({
        url: process.env["DASSIE_DEV_RPC_URL"]!,
        WebSocket,
      }),
      transformer: superjson,
    })

    sig.onCleanup(() => {
      rpcClient.close()
    })

    return rpcClient.rpc
  })
