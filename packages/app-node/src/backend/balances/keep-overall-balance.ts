import { createActor } from "@dassie/lib-reactive"

import { overallBalanceSignal } from "./signals/overall-balance-signal"
import { peerBalanceMapStore } from "./stores/peer-balance-map"

export const keepOverallBalance = () =>
  createActor((sig) => {
    const peerBalanceMap = sig.get(peerBalanceMapStore)

    let totalBalance = 0n

    for (const { balance } of peerBalanceMap.values()) {
      totalBalance += balance
    }

    sig.use(overallBalanceSignal).write(totalBalance)
  })
