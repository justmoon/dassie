import { createActor } from "@dassie/lib-reactive"

import {
  cleanupPeer,
  initializePeer,
} from "../accounting/functions/manage-peer"
import { ledgerStore } from "../accounting/stores/ledger"
import { settlePerPeer } from "../settlement"
import type { PerSubnetParameters } from "../subnets/manage-subnet-instances"
import { NodeTableKey } from "./stores/node-table"

export interface PerPeerParameters extends PerSubnetParameters {
  peerId: NodeTableKey
}

export const runPerPeerActors = () =>
  createActor((sig, parameters: PerPeerParameters) => {
    const { subnetId, peerId } = parameters

    const ledger = sig.use(ledgerStore)

    initializePeer(ledger, subnetId, peerId)

    sig.run(settlePerPeer, parameters)

    sig.onCleanup(() => {
      cleanupPeer(ledger, peerId)
    })
  })
