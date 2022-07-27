import { useState } from "react"
import { QueryClient, QueryClientProvider } from "react-query"
import { Route } from "wouter"

import Dashboard from "./components/pages/dashboard"
import Logs from "./components/pages/logs"
import NodeDetail from "./components/pages/node-detail"
import MainNavigation from "./main-navigation"
import { trpc, client as trpcClient } from "./utils/trpc"

const App = () => {
  const [queryClient] = useState(() => new QueryClient())

  return (
    <trpc.Provider client={trpcClient} queryClient={queryClient}>
      <QueryClientProvider client={queryClient}>
        <MainNavigation />
        <div className="min-h-screen pl-64">
          <Route path="/">
            <Dashboard />
          </Route>
          <Route path="/logs">
            <Logs />
          </Route>
          <Route path="/nodes/:nodeId">
            {({ nodeId }) => <NodeDetail nodeId={nodeId} />}
          </Route>
        </div>
      </QueryClientProvider>
    </trpc.Provider>
  )
}

export default App
