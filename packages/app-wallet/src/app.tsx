import { useSig } from "@dassie/lib-reactive-react"

import { rootEffect } from "./effects/root"
import { Account } from "./pages/account/account"
import { CreateFirstAccount } from "./pages/create-first-account/create-first-account"
import { Open } from "./pages/open/open"
import { walletStore } from "./stores/wallet"

const App = () => {
  const sig = useSig()
  sig.run(rootEffect)

  const wallet = sig.get(walletStore)

  if (wallet.seed == undefined) {
    return <Open />
  }

  if (wallet.accounts.length === 0) {
    return <CreateFirstAccount />
  }

  return <Account />
}

export default App
