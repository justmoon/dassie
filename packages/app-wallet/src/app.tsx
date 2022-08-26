import { useWallet } from "./hooks/use-wallet"
import { Account } from "./pages/account/account"
import { CreateFirstAccount } from "./pages/create-first-account/create-first-account"
import { Open } from "./pages/open/open"

const App = () => {
  const wallet = useWallet()

  if (wallet.seed == undefined) {
    return <Open />
  }

  if (wallet.accounts.length === 0) {
    return <CreateFirstAccount />
  }

  return <Account />
}

export default App
