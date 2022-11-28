import produce, { enableMapSet } from "immer"
import type { MarkOptional } from "ts-essentials"

import { createStore } from "@dassie/lib-reactive"

enableMapSet()

export interface PeerEntry {
  subnetId: string
  nodeId: string
  url: string
  lastSeen: number
}

export type NewPeerEntry = MarkOptional<PeerEntry, "lastSeen">

export type PeerTableKey = `${string}.${string}`

export const peerTableStore = () =>
  createStore(new Map<PeerTableKey, PeerEntry>(), {
    addPeer: (peerEntry: NewPeerEntry) =>
      produce((draft) => {
        draft.set(`${peerEntry.subnetId}.${peerEntry.nodeId}`, {
          lastSeen: Date.now(),
          ...peerEntry,
        })
      }),
    updatePeer: (key: PeerTableKey, peerEntry: Partial<PeerEntry>) =>
      produce((draft) => {
        const previousEntry = draft.get(key)
        if (previousEntry == undefined) {
          throw new Error("nodeId not found")
        }
        draft.set(key, {
          ...previousEntry,
          ...peerEntry,
        })
      }),
  })
