import { createActor } from "@dassie/lib-reactive"

import { settlePerPeer } from "../settlement"
import { createPeerLedgerEntries } from "./create-peer-ledger-entries"
import { NodeId } from "./types/node-id"

export const runPerPeerActors = () =>
  createActor((sig, peerId: NodeId) => {
    sig.run(createPeerLedgerEntries, peerId)
    sig.run(settlePerPeer, peerId)
  })
