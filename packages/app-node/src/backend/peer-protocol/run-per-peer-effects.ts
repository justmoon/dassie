import { createActor } from "@dassie/lib-reactive"

import { peerBalanceMapStore } from "../balances/stores/peer-balance-map"
import type { NodeTableKey } from "./stores/node-table"

export const runPerPeerEffects = () =>
  createActor((sig, peerKey: NodeTableKey) => {
    sig.use(peerBalanceMapStore).initializePeer(peerKey)

    sig.onCleanup(() => {
      sig.use(peerBalanceMapStore).removePeer(peerKey)
    })
  })
