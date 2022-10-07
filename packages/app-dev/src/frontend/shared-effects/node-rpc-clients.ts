import { createTRPCProxyClient } from "@trpc/client"
import { createWSClient, wsLink } from "@trpc/client/links/wsLink"
import { keyedObjectReffx } from "reffx/lib"
import superjson from "superjson"

import type { DebugRpcRouter } from "../../runner/effects/debug-rpc-server"
import { nodeIdToDebugUrl } from "../utils/node-id-to-debug-port"

export const nodeClients = keyedObjectReffx((nodeId: string) => {
  const wsClient = createWSClient({
    url: nodeIdToDebugUrl(nodeId),
  })

  const client = createTRPCProxyClient<DebugRpcRouter>({
    transformer: superjson,
    links: [
      wsLink({
        client: wsClient,
      }),
    ],
  })

  return [
    client,
    () => {
      // Wait a moment to disconnect - this gives the subscriptions a chance to cleanly unsubscribe
      setTimeout(() => {
        wsClient.close()
      })
    },
  ] as const
})
