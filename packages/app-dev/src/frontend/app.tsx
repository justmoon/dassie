import { Route } from "wouter"

import Dashboard from "./components/pages/dashboard/dashboard"
import Logs from "./components/pages/logs"
import NodeDetail from "./components/pages/node-detail"
import MainNavigation from "./main-navigation"
import { activeNodesStore } from "./remote-signals/active-nodes"
import { environmentSettingsStore } from "./remote-signals/environment-settings"
import { remoteLogsStore } from "./remote-signals/logs"
import { peeringStateStore } from "./remote-signals/peering-state"
import { peerTrafficTopic } from "./remote-topics/peer-traffic"
import { useSig } from "./utils/remote-reactive"

const App = () => {
  const sig = useSig()
  sig.run(activeNodesStore, undefined, { register: true })
  sig.run(remoteLogsStore, undefined, { register: true })
  sig.run(peerTrafficTopic, undefined, { register: true })
  sig.run(peeringStateStore, undefined, { register: true })
  sig.run(environmentSettingsStore, undefined, { register: true })

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
