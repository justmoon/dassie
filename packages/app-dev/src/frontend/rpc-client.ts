import { createTRPCClient } from "@trpc/client"
import { createWSClient, wsLink } from "@trpc/client/links/wsLink"

import type { UiRpcRouter } from "../servers/ui-rpc-server"

export const wsClient = createWSClient({ url: "ws://localhost:10001" })

const client = createTRPCClient<UiRpcRouter>({
  links: [wsLink({ client: wsClient })],
})

export default client
