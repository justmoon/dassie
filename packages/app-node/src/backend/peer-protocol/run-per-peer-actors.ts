import { createActor } from "@dassie/lib-reactive"

import {
  cleanupPeer,
  initializePeer,
} from "../accounting/functions/manage-peer"
import { ledgerStore } from "../accounting/stores/ledger"
import { settlePerPeer } from "../settlement"
import type { PerSubnetParameters } from "../subnets/manage-subnet-instances"
import type { NodeTableKey } from "./stores/node-table"

export interface PerPeerParameters extends PerSubnetParameters {
  peerKey: NodeTableKey
}

export const runPerPeerActors = () =>
  createActor((sig, parameters: PerPeerParameters) => {
    const { peerKey } = parameters

    const ledger = sig.use(ledgerStore)

    initializePeer(ledger, peerKey)

    sig.run(settlePerPeer, parameters)

    sig.onCleanup(() => {
      cleanupPeer(ledger, peerKey)
    })
  })
