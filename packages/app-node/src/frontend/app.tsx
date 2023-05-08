import { Route, Switch } from "wouter"

import { rootActor } from "./actors/root"
import { Account } from "./pages/account/account"
import { CreateFirstAccount } from "./pages/create-first-account/create-first-account"
import { Open } from "./pages/open/open"
import { PaymentStatus } from "./pages/payment-status/payment-status"
import { Send } from "./pages/send/send"
import { walletStore } from "./stores/wallet"
import { useSig } from "./utils/remote-reactive"

const App = () => {
  const sig = useSig()
  sig.run(rootActor)

  const wallet = sig.get(walletStore)

  if (wallet.seed == undefined) {
    return <Open />
  }

  if (wallet.accounts.length === 0) {
    return <CreateFirstAccount />
  }

  return (
    <Switch>
      <Route path="/send" component={Send} />
      <Route path="/payments/:paymentId" component={PaymentStatus} />
      <Route component={Account} />
    </Switch>
  )
}

export default App
