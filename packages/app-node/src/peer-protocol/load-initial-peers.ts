import type { EffectContext } from "@dassie/lib-reactive"

import { configStore } from "../config"
import { addPeer, peerTableStore } from "./stores/peer-table"

export const loadInitialPeers = (sig: EffectContext) => {
  const initialPeers = sig.get(configStore, (config) => config.initialPeers)

  for (const peer of initialPeers) {
    sig.use(peerTableStore).update(addPeer(peer))
  }
}
