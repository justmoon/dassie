import { Route, Switch } from "wouter"

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
        <Route path="/" component={Dashboard} />
        <Route path="/logs" component={Logs} />
        <Route path="/scenarios" component={Scenarios} />
        <Switch>
          <Route path="/nodes/host" nest component={HostDetail} />
          <Route path="/nodes/:nodeId" nest component={NodeDetail} />
        </Switch>
      </div>
    </div>
  )
}

export default App
