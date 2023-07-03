import { useState } from "react"

import { USD_SPECIFICATION } from "../constants/currency"
import { trpc } from "../utils/trpc"

export const useAccount = () => {
  const [balance, setBalance] = useState(0n)

  trpc.general.subscribeBalance.useSubscription(undefined, {
    onData: (data) => {
      setBalance(data)
    },
  })

  return {
    currency: USD_SPECIFICATION,
    balance,
  }
}
