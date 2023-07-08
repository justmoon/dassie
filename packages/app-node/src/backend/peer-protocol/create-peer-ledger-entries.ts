import assert from "node:assert"

import { createActor } from "@dassie/lib-reactive"

import {
  cleanupPeer,
  initializePeer,
} from "../accounting/functions/manage-peer"
import { ledgerStore } from "../accounting/stores/ledger"
import { nodeTableStore } from "./stores/node-table"
import { NodeId } from "./types/node-id"

export const createPeerLedgerEntries = () =>
  createActor((sig, peerId: NodeId) => {
    const ledger = sig.use(ledgerStore)
    const nodeTable = sig.use(nodeTableStore)

    const peerState = nodeTable.read().get(peerId)?.peerState

    assert(peerState?.id === "peered", "peer state must be 'peered'")

    const { settlementSchemeId } = peerState

    initializePeer(ledger, settlementSchemeId, peerId)

    sig.onCleanup(() => {
      cleanupPeer(ledger, peerId)
    })
  })
