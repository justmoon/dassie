import type { EffectContext } from "@xen-ilp/lib-reactive"

import { configStore } from "../config"
import { addPeer, peerTableStore } from "./stores/peer-table"

export const initialPeerLoader = (sig: EffectContext) => {
  const initialPeers = sig.get(configStore, (config) => config.initialPeers)

  for (const peer of initialPeers) {
    sig.reactor.emit(peerTableStore, addPeer(peer))
  }
}
