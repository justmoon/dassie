import { Route } from "wouter"

import Dashboard from "./components/pages/dashboard/dashboard"
import HostDetail from "./components/pages/host-detail"
import Logs from "./components/pages/logs"
import NodeDetail from "./components/pages/node-detail"
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
        <Route path="/nodes/:nodeId">
          {({ nodeId }) =>
            nodeId === "host" ? (
              <HostDetail />
            ) : nodeId ? (
              <NodeDetail nodeId={nodeId} />
            ) : null
          }
        </Route>
      </div>
    </div>
  )
}

export default App
