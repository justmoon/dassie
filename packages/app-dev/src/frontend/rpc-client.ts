import { createTRPCClient } from "@trpc/client"
import { createWSClient, wsLink } from "@trpc/client/links/wsLink"
import superjson from "superjson"

import type { AppRouter } from "../backend/rpc-routers/app-router"

export const wsClient = createWSClient({ url: "ws://localhost:10001" })

const client = createTRPCClient<AppRouter>({
  links: [wsLink({ client: wsClient })],
  transformer: superjson,
})

export default client
