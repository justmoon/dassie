import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { useState } from "react"
import superjson from "superjson"

import { DarkModeProvider } from "@dassie/app-node/src/frontend/components/context/dark-mode"

import App from "./app"
import { trpc, wsLink } from "./utils/trpc"

const Root = () => {
  const [queryClient] = useState(() => new QueryClient())
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
