/* eslint-disable @typescript-eslint/no-explicit-any */
import { createTRPCProxyClient } from "@trpc/client"
import { createWSClient, wsLink } from "@trpc/client/links/wsLink"
import superjson from "superjson"
import WebSocket from "ws"

import { createService } from "@dassie/lib-reactive"

import type { AppRouter } from "../../backend/rpc-routers/app-router"

export const trpcClientService = () =>
  createService((sig) => {
    // BUG in @trpc/client/links/wsLink: Theoretically, we're supposed to be able to pass a WebSocket implementation to createWSClient, but it tries to access the one on the global object anyway.
    // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
    global.WebSocket = WebSocket as any

    const wsClient = createWSClient({
      url: process.env["DASSIE_DEV_RPC_URL"]!,
      // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
      WebSocket: WebSocket as any,
    })

    const trpcClient = createTRPCProxyClient<AppRouter>({
      links: [
        wsLink({
          client: wsClient,
        }),
      ],
      transformer: superjson,
    })

    sig.onCleanup(() => {
      wsClient.close()
    })

    return trpcClient
  })
