import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import {
  createTRPCClientProxy,
  createWSClient,
  wsLink as createWSLink,
} from "@trpc/react-query"
import { useState } from "react"

import type { AppRouter } from "../backend/trpc-server/app-router"
import App from "./app"
import { Provider as RemoteReactiveProvider } from "./utils/remote-reactive"
import { trpc } from "./utils/trpc"

const Root = () => {
  const [queryClient] = useState(() => new QueryClient())
  const [wsLink] = useState(() => {
    const wsClient = createWSClient({
      url: `${__DASSIE_NODE_URL__.replace(/^http/, "ws")}trpc`,
    })
    return createWSLink({ client: wsClient })
  })
  const [trpcClient] = useState(() => {
    return trpc.createClient({
      links: [wsLink],
    })
  })
  const [trpcProxyClient] = useState(() => {
    return createTRPCClientProxy<AppRouter>(trpcClient)
  })

  return (
    <RemoteReactiveProvider trpcClient={trpcProxyClient}>
      <trpc.Provider client={trpcClient} queryClient={queryClient}>
        <QueryClientProvider client={queryClient}>
          <App />
        </QueryClientProvider>
      </trpc.Provider>
    </RemoteReactiveProvider>
  )
}

export default Root
