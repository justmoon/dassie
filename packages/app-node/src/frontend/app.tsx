import { Route, Switch, useRoute } from "wouter"

import { MainNavigation } from "./layout/main-navigation/main-navigation"
import { Account } from "./pages/account/account"
import { CreateFirstAccount } from "./pages/create-first-account/create-first-account"
import { Database } from "./pages/debug/database/database"
import { Ledger } from "./pages/debug/ledger/ledger"
import { Nodes } from "./pages/debug/nodes/nodes"
import { Routing } from "./pages/debug/routing/routing"
import { State } from "./pages/debug/state/state"
import { LoginPage } from "./pages/login/login"
import { PaymentStatus } from "./pages/payment-status/payment-status"
import { Send } from "./pages/send/send"
import { Settings } from "./pages/settings/settings"
import { Setup } from "./pages/setup/setup"
import { SetupInfoPage } from "./pages/setup/setup-info"
import { trpc } from "./utils/trpc"

const App = () => {
  const { data: basicState } = trpc.general.getBasicState.useQuery()
  const [isSetupRoute, setupRouteParameters] = useRoute("/setup/:token")

  if (!basicState) {
    return null
  }

  if (basicState.state === "uninitialized") {
    return isSetupRoute ? (
      <Setup token={setupRouteParameters.token} />
    ) : (
      <SetupInfoPage />
    )
  }

  if (basicState.state === "anonymous") {
    return <LoginPage />
  }

  if (basicState.activeSettlementSchemes.length === 0) {
    return <CreateFirstAccount />
  }

  return (
    <div className="h-screen grid grid-rows-[auto_1fr]">
      <MainNavigation />
      <Switch>
        <Route path="/send" component={Send} />
        <Route path="/payments/:paymentId" component={PaymentStatus} />
        <Route path="/debug/ledger" component={Ledger} />
        <Route path="/debug/nodes" component={Nodes} />
        <Route path="/debug/routing" component={Routing} />
        <Route path="/debug/state" component={State} />
        <Route path="/debug/database" component={Database} />
        <Route path="/settings" component={Settings} />
        <Route component={Account} />
      </Switch>
    </div>
  )
}

export default App
