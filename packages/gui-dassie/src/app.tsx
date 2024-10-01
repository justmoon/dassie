import { Route, Switch, useRoute } from "wouter"

import { MainNavigation } from "./layout/main-navigation/main-navigation"
import { Account } from "./pages/account/account"
import { CreateFirstAccount } from "./pages/create-first-account/create-first-account"
import { DebugPage } from "./pages/debug/debug"
import { LedgersPage } from "./pages/ledgers/ledgers"
import { LoginPage } from "./pages/login/login"
import { PaymentStatus } from "./pages/payment-status/payment-status"
import { ReceivePage } from "./pages/receive/receive"
import { Send } from "./pages/send/send"
import { Settings } from "./pages/settings/settings"
import { Setup } from "./pages/setup/setup"
import { SetupInfoPage } from "./pages/setup/setup-info"
import { rpc } from "./utils/rpc"

const App = () => {
  const { data: basicState } = rpc.general.getBasicState.useQuery()
  const [isSetupRoute, setupRouteParameters] = useRoute("/setup/:token")

  if (!basicState) {
    return null
  }

  if (basicState.state === "uninitialized") {
    return isSetupRoute ?
        <Setup token={setupRouteParameters.token} />
      : <SetupInfoPage />
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
        <Route path="/ledgers" component={LedgersPage} />
        <Route path="/send" component={Send} />
        <Route path="/receive" component={ReceivePage} />
        <Route path="/payments/:paymentId" component={PaymentStatus} />
        <Route path="/debug" component={DebugPage} nest />
        <Route path="/settings" component={Settings} />
        <Route component={Account} />
      </Switch>
    </div>
  )
}

export default App
