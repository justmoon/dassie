import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { createWSClient, wsLink as createWSLink } from "@trpc/react-query"
import { useState } from "react"
import superjson from "superjson"

import App from "./app"
import { DarkModeProvider } from "./components/context/dark-mode"
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
      transformer: superjson,
    })
  })

  return (
    <DarkModeProvider>
      <trpc.Provider client={trpcClient} queryClient={queryClient}>
        <QueryClientProvider client={queryClient}>
          <App />
        </QueryClientProvider>
      </trpc.Provider>
    </DarkModeProvider>
  )
}

export default Root
