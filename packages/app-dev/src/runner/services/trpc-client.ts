/* eslint-disable @typescript-eslint/no-explicit-any, @typescript-eslint/no-unsafe-member-access */
import { TRPCClient, createTRPCClient } from "@trpc/client"
import { createWSClient, wsLink } from "@trpc/client/links/wsLink"
import superjson from "superjson"
import WebSocket from "ws"

import type { AppRouter } from "../../backend/rpc-routers/app-router"

export const trpcClientFactory = () => {
  if ((global as any).trpcClient)
    return (global as any).trpcClient as TRPCClient<AppRouter>

  const wsClient = createWSClient({
    url: process.env["XEN_DEV_RPC_URL"]!,
    // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment, @typescript-eslint/no-explicit-any
    WebSocket: WebSocket as any,
  })

  const trpcClient = createTRPCClient<AppRouter>({
    links: [
      wsLink({
        client: wsClient,
      }),
    ],
    transformer: superjson,
  })

  return trpcClient
}
