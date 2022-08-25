import { useWallet } from "./hooks/use-wallet"
import { Open } from "./pages/open/open"

const App = () => {
  const wallet = useWallet()

  if (!wallet.open) {
    return <Open />
  }

  return <div>Your wallet is set up!</div>
}

export default App
