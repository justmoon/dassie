import produce, { enableMapSet } from "immer"
import type { MarkOptional } from "ts-essentials"

import { createStore } from "@dassie/lib-reactive"

enableMapSet()

export interface PeerEntry {
  nodeId: string
  url: string
  lastSeen: number
}

export type NewPeerEntry = MarkOptional<PeerEntry, "lastSeen">

export const peerTableStore = () =>
  createStore(new Map<string, PeerEntry>(), {
    addPeer: (peerEntry: NewPeerEntry) =>
      produce((draft) => {
        draft.set(peerEntry.nodeId, {
          lastSeen: Date.now(),
          ...peerEntry,
        })
      }),
    updatePeer: (nodeId: string, peerEntry: Partial<PeerEntry>) =>
      produce((draft) => {
        const previousEntry = draft.get(nodeId)
        if (previousEntry == undefined) {
          throw new Error("nodeId not found")
        }
        draft.set(nodeId, {
          ...previousEntry,
          ...peerEntry,
        })
      }),
  })
