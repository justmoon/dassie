import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { useState } from "react"

import { DarkModeProvider } from "@dassie/app-node/src/frontend/components/context/dark-mode"

import App from "./app"
import { RpcProvider, clientOptions, useWebSocketClient } from "./utils/rpc"

const Root = () => {
  const [queryClient] = useState(() => new QueryClient())
  const rpcClient = useWebSocketClient({
    url: "wss://dev-rpc.localhost",
    clientOptions,
  })

  return (
    <DarkModeProvider>
      <RpcProvider rpcClient={rpcClient} queryClient={queryClient}>
        <QueryClientProvider client={queryClient}>
          <App />
        </QueryClientProvider>
      </RpcProvider>
    </DarkModeProvider>
  )
}

export default Root
