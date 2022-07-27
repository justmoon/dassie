import { createTRPCClient } from "@trpc/client"
import { createWSClient, wsLink } from "@trpc/client/links/wsLink"
import { createReactQueryHooks } from "@trpc/react"
import superjson from "superjson"

import type { AppRouter } from "../../backend/rpc-routers/app-router"

export const trpc = createReactQueryHooks<AppRouter>()

export const wsClient = createWSClient({ url: "ws://localhost:10001" })

export const client = createTRPCClient<AppRouter>({
  links: [wsLink({ client: wsClient })],
  transformer: superjson,
})
