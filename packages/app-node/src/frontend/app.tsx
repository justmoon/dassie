import { Route, Switch } from "wouter"

import { MainNavigation } from "./layout/main-navigation"
import { Account } from "./pages/account/account"
import { CreateFirstAccount } from "./pages/create-first-account/create-first-account"
import { Ledger } from "./pages/debug/ledger/ledger"
import { Nodes } from "./pages/debug/nodes/nodes"
import { Routing } from "./pages/debug/routing/routing"
import { LoginPage } from "./pages/login/login"
import { Open } from "./pages/open/open"
import { PaymentStatus } from "./pages/payment-status/payment-status"
import { Send } from "./pages/send/send"
import { Settings } from "./pages/settings/settings"
import { trpc } from "./utils/trpc"

const App = () => {
  const { data: basicState } = trpc.general.getBasicState.useQuery()

  if (!basicState) {
    return null
  }

  if (basicState.state === "uninitialized") {
    return <Open />
  }

  if (basicState.state === "anonymous") {
    return <LoginPage />
  }

  if (basicState.activeSettlementSchemes.length === 0) {
    return <CreateFirstAccount />
  }

  return (
    <div>
      <MainNavigation />
      <Switch>
        <Route path="/send" component={Send} />
        <Route path="/payments/:paymentId" component={PaymentStatus} />
        <Route path="/debug/ledger" component={Ledger} />
        <Route path="/debug/nodes" component={Nodes} />
        <Route path="/debug/routing" component={Routing} />
        <Route path="/settings" component={Settings} />
        <Route component={Account} />
      </Switch>
    </div>
  )
}

export default App
