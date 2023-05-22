import { Route, Switch } from "wouter"

import { MainNavigation } from "./layout/main-navigation"
import { Account } from "./pages/account/account"
import { CreateFirstAccount } from "./pages/create-first-account/create-first-account"
import { Ledger } from "./pages/debug/ledger/ledger"
import { Open } from "./pages/open/open"
import { PaymentStatus } from "./pages/payment-status/payment-status"
import { Send } from "./pages/send/send"
import { walletStore } from "./stores/wallet"

const App = () => {
  const { seed } = walletStore

  if (seed.value == undefined) {
    return <Open />
  }

  // TODO: Get accounts from server
  const accounts = ["stub"]

  if (accounts.length === 0) {
    return <CreateFirstAccount />
  }

  return (
    <div>
      <MainNavigation />
      <Switch>
        <Route path="/send" component={Send} />
        <Route path="/payments/:paymentId" component={PaymentStatus} />
        <Route path="/debug/ledger" component={Ledger} />
        <Route component={Account} />
      </Switch>
    </div>
  )
}

export default App
