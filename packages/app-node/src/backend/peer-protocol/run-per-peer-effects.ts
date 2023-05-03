import { createActor } from "@dassie/lib-reactive"

import { peerBalanceMapStore } from "../balances/stores/peer-balance-map"
import type { PerSubnetParameters } from "../subnets/manage-subnet-instances"
import type { NodeTableKey } from "./stores/node-table"

export interface PerPeerParameters extends PerSubnetParameters {
  peerKey: NodeTableKey
}

export const runPerPeerEffects = () =>
  createActor((sig, { peerKey }: PerPeerParameters) => {
    sig.use(peerBalanceMapStore).initializePeer(peerKey)

    sig.onCleanup(() => {
      sig.use(peerBalanceMapStore).removePeer(peerKey)
    })
  })
