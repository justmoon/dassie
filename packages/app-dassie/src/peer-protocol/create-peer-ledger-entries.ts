import { assert } from "@dassie/lib-logger"
import { type Reactor, createActor, createMapped } from "@dassie/lib-reactive"

import {
  cleanupPeer,
  initializePeer,
} from "../accounting/functions/manage-peer"
import { LedgerStore } from "../accounting/stores/ledger"
import { GetLedgerIdForSettlementScheme } from "../ledgers/functions/get-ledger-id"
import { peerProtocol as logger } from "../logger/instances"
import { PeersSignal } from "./computed/peers"
import { NodeTableStore } from "./stores/node-table"

export const CreatePeerLedgerEntriesActor = (reactor: Reactor) => {
  const ledger = reactor.use(LedgerStore)
  const nodeTable = reactor.use(NodeTableStore)
  const getLedgerIdForSettlementScheme = reactor.use(
    GetLedgerIdForSettlementScheme,
  )

  return createMapped(reactor, PeersSignal, (peerId) =>
    createActor((sig) => {
      const peerState = nodeTable.read().get(peerId)?.peerState

      assert(logger, peerState?.id === "peered", "peer state must be 'peered'")

      const { settlementSchemeId } = peerState

      const ledgerId = getLedgerIdForSettlementScheme(settlementSchemeId)

      initializePeer(ledger, ledgerId, peerId)

      sig.onCleanup(() => {
        cleanupPeer(ledger, peerId)
      })
    }),
  )
}
