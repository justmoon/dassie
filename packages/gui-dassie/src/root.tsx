import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { useState } from "react"

import App from "./app"
import { DarkModeProvider } from "./components/context/dark-mode"
import { Toaster } from "./components/ui/toast/toaster"
import { RpcProvider, clientOptions, useWebSocketClient } from "./utils/rpc"

const Root = () => {
  const wsUrl = new URL("/", location.href)
  wsUrl.protocol = "wss:"
  wsUrl.pathname = "/rpc"

  const [queryClient] = useState(() => new QueryClient())
  const rpcClient = useWebSocketClient({
    url: wsUrl.toString(),
    clientOptions,
  })

  return (
    <DarkModeProvider>
      <RpcProvider rpcClient={rpcClient}>
        <QueryClientProvider client={queryClient}>
          <App />
          <Toaster />
        </QueryClientProvider>
      </RpcProvider>
    </DarkModeProvider>
  )
}

export default Root
