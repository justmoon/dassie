import assert from "node:assert"

import { Reactor, createActor, createMapped } from "@dassie/lib-reactive"

import {
  cleanupPeer,
  initializePeer,
} from "../accounting/functions/manage-peer"
import { LedgerStore } from "../accounting/stores/ledger"
import { PeersSignal } from "./computed/peers"
import { NodeTableStore } from "./stores/node-table"

export const CreatePeerLedgerEntriesActor = (reactor: Reactor) => {
  const ledger = reactor.use(LedgerStore)
  const nodeTable = reactor.use(NodeTableStore)

  return createMapped(reactor, reactor.use(PeersSignal), (peerId) =>
    createActor((sig) => {
      const peerState = nodeTable.read().get(peerId)?.peerState

      assert(peerState?.id === "peered", "peer state must be 'peered'")

      const { settlementSchemeId } = peerState

      initializePeer(ledger, settlementSchemeId, peerId)

      sig.onCleanup(() => {
        cleanupPeer(ledger, peerId)
      })
    }),
  )
}
