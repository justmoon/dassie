import { Route } from "wouter"

import Dashboard from "./components/pages/dashboard/dashboard"
import Logs from "./components/pages/logs"
import NodeDetail from "./components/pages/node-detail"
import MainNavigation from "./main-navigation"
import { activeNodesStore } from "./remote-signals/active-nodes"
import { remoteLogsStore } from "./remote-signals/logs"
import { peeringStateStore } from "./remote-signals/peering-state"
import { peerTrafficTopic } from "./remote-topics/peer-traffic"
import { useSig } from "./utils/remote-reactive"

const App = () => {
  const sig = useSig()
  sig.run(sig.use(activeNodesStore).effect)
  sig.run(sig.use(remoteLogsStore).effect)
  sig.run(sig.use(peerTrafficTopic).effect)
  sig.run(sig.use(peeringStateStore).effect)

  return (
    <>
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
    </>
  )
}

export default App
