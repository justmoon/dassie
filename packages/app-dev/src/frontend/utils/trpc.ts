import { createTRPCProxyClient } from "@trpc/client"
import {
  createWSClient,
  wsLink as createWSLink,
} from "@trpc/client/links/wsLink"
import { createTRPCReact } from "@trpc/react"
import superjson from "superjson"

import type { AppRouter } from "../../backend/rpc-routers/app-router"

export const trpc = createTRPCReact<AppRouter>()

export const wsClient = createWSClient({ url: "wss://dev-rpc.localhost" })
export const wsLink = createWSLink({ client: wsClient })

export const client = createTRPCProxyClient<AppRouter>({
  links: [wsLink],
  transformer: superjson,
})
