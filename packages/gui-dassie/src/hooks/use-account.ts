import { useState } from "react"

import { USD_SPECIFICATION } from "../constants/currency"
import { rpc } from "../utils/rpc"

export const useAccount = () => {
  const [balance, setBalance] = useState(0n)

  rpc.general.subscribeBalance.useSubscription(undefined, {
    onData: (data) => {
      setBalance(data)
    },
  })

  return {
    currency: USD_SPECIFICATION,
    balance,
  }
}
