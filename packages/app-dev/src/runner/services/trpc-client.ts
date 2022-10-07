/* eslint-disable @typescript-eslint/no-explicit-any, @typescript-eslint/no-unsafe-member-access */
import { createTRPCProxyClient } from "@trpc/client"
import { createWSClient, wsLink } from "@trpc/client/links/wsLink"
import superjson from "superjson"
import WebSocket from "ws"

import { createService } from "@dassie/lib-reactive"

import type { AppRouter } from "../../backend/rpc-routers/app-router"

export const trpcClientService = () =>
  createService((sig) => {
    const wsClient = createWSClient({
      url: process.env["DASSIE_DEV_RPC_URL"]!,
      // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment, @typescript-eslint/no-explicit-any
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
