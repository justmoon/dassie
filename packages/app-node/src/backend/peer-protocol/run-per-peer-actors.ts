import { createActor, createMapped } from "@dassie/lib-reactive"

import { settlePerPeer } from "../settlement"
import { peersComputation } from "./computed/peers"
import { createPeerLedgerEntries } from "./create-peer-ledger-entries"

export const runPerPeerActors = () =>
  createMapped(peersComputation, (peerId) =>
    createActor((sig) => {
      sig.run(createPeerLedgerEntries, peerId)
      sig.run(settlePerPeer, peerId)
    })
  )
