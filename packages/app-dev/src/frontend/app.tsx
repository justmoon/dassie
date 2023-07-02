import { Route } from "wouter"

import Dashboard from "./components/pages/dashboard/dashboard"
import Logs from "./components/pages/logs"
import NodeDetail from "./components/pages/node-detail"
import MainNavigation from "./main-navigation"

const App = () => {
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
