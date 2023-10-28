import assert from "node:assert"

import { createActor } from "@dassie/lib-reactive"

import {
  cleanupPeer,
  initializePeer,
} from "../accounting/functions/manage-peer"
import { LedgerStore } from "../accounting/stores/ledger"
import { NodeTableStore } from "./stores/node-table"
import { NodeId } from "./types/node-id"

export const CreatePeerLedgerEntriesActor = () =>
  createActor((sig, peerId: NodeId) => {
    const ledger = sig.use(LedgerStore)
    const nodeTable = sig.use(NodeTableStore)

    const peerState = nodeTable.read().get(peerId)?.peerState

    assert(peerState?.id === "peered", "peer state must be 'peered'")

    const { settlementSchemeId } = peerState

    initializePeer(ledger, settlementSchemeId, peerId)

    sig.onCleanup(() => {
      cleanupPeer(ledger, peerId)
    })
  })
