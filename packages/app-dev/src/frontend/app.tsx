import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { useState } from "react"
import superjson from "superjson"
import { Route } from "wouter"

import { useSig } from "@dassie/lib-reactive-trpc/client"

import Dashboard from "./components/pages/dashboard"
import Logs from "./components/pages/logs"
import NodeDetail from "./components/pages/node-detail"
import MainNavigation from "./main-navigation"
import { activeNodesStore } from "./remote-signals/active-nodes"
import { remoteLogsStore } from "./remote-signals/logs"
import { peerTrafficTopic } from "./remote-topics/peer-traffic"
import { trpcConnectionService } from "./utils/remote-reactive"
import { trpc, wsLink } from "./utils/trpc"

const App = () => {
  const sig = useSig()
  sig.run(sig.use(trpcConnectionService).effect)
  sig.run(sig.use(activeNodesStore).effect)
  sig.run(sig.use(remoteLogsStore).effect)
  sig.run(sig.use(peerTrafficTopic).effect)

  const [queryClient] = useState(() => new QueryClient())
  const [trpcClient] = useState(() => {
    return trpc.createClient({
      links: [wsLink],
      transformer: superjson,
    })
  })

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
            {({ nodeId }) => (nodeId ? <NodeDetail nodeId={nodeId} /> : null)}
          </Route>
        </div>
      </QueryClientProvider>
    </trpc.Provider>
  )
}

export default App
