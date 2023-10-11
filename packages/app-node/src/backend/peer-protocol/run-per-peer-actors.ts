import { Reactor, createActor, createMapped } from "@dassie/lib-reactive"

import { SettlePerPeerActor } from "../settlement"
import { PeersSignal } from "./computed/peers"
import { CreatePeerLedgerEntriesActor } from "./create-peer-ledger-entries"

export const PerPeerActors = (reactor: Reactor) =>
  createMapped(reactor.lifecycle, reactor.use(PeersSignal), (peerId) =>
    createActor((sig) => {
      sig.run(CreatePeerLedgerEntriesActor, peerId)
      sig.run(SettlePerPeerActor, peerId)
    }),
  )
