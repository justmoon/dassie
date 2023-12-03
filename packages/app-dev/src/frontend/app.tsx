import { Route } from "wouter"

import Dashboard from "./components/pages/dashboard/dashboard"
import Logs from "./components/pages/logs/logs"
import HostDetail from "./components/pages/nodes/host-detail"
import NodeDetail from "./components/pages/nodes/node-detail"
import { Scenarios } from "./components/pages/scenarios/scenarios"
import MainNavigation from "./main-navigation"

const App = () => {
  return (
    <div className="grid grid-cols-[auto_1fr] min-h-screen">
      <MainNavigation />
      <div>
        <Route path="/">
          <Dashboard />
        </Route>
        <Route path="/logs">
          <Logs />
        </Route>
        <Route path="/scenarios">
          <Scenarios />
        </Route>
        <Route path="/nodes/:nodeId/:params*">
          {({ nodeId, params }) =>
            nodeId === "host" ? (
              <HostDetail />
            ) : nodeId ? (
              <NodeDetail nodeId={nodeId} secondaryParameters={params} />
            ) : null
          }
        </Route>
      </div>
    </div>
  )
}

export default App
