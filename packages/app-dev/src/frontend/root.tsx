import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { createTRPCClientProxy } from "@trpc/client"
import { useState } from "react"
import superjson from "superjson"

import type { AppRouter } from "../backend/rpc-routers/app-router"
import App from "./app"
import { Provider as RemoteReactiveProvider } from "./utils/remote-reactive"
import { trpc, wsLink } from "./utils/trpc"

const Root = () => {
  const [queryClient] = useState(() => new QueryClient())
  const [trpcClient] = useState(() => {
    return trpc.createClient({
      links: [wsLink],
      transformer: superjson,
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
