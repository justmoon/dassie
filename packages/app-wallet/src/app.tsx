import { useWallet } from "./hooks/use-wallet"
import { FirstSubnet } from "./pages/first-subnet/first-subnet"
import { Open } from "./pages/open/open"

const App = () => {
  const wallet = useWallet()

  if (wallet.seed == undefined) {
    return <Open />
  }

  if (wallet.subnets.length === 0) {
    return <FirstSubnet />
  }

  return <div>Your wallet is set up!</div>
}

export default App
