import { createActor } from "@dassie/lib-reactive"

import { settlePerPeer } from "../../settlement"
import { peerBalanceMapStore } from "../balances/stores/peer-balance-map"
import type { PerSubnetParameters } from "../subnets/manage-subnet-instances"
import type { NodeTableKey } from "./stores/node-table"

export interface PerPeerParameters extends PerSubnetParameters {
  peerKey: NodeTableKey
}

export const runPerPeerEffects = () =>
  createActor((sig, parameters: PerPeerParameters) => {
    const { peerKey } = parameters

    sig.use(peerBalanceMapStore).initializePeer(peerKey)

    sig.run(settlePerPeer, parameters)

    sig.onCleanup(() => {
      sig.use(peerBalanceMapStore).removePeer(peerKey)
    })
  })
