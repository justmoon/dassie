import { useState } from "react"

import { trpc } from "../utils/trpc"

export const useAccount = () => {
  const [balance, setBalance] = useState(0n)

  trpc.subscribeBalance.useSubscription(undefined, {
    onData: (data) => {
      setBalance(BigInt(data))
    },
  })

  return {
    currency: {
      symbol: "$",
      code: "USD",
      precision: 2,
      totalPrecision: 9,
    },
    balance,
  }
}
