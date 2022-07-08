import produce from "immer"

import { createStore } from "@xen-ilp/lib-reactive"

export interface PeerEntry {
  nodeId: string
  url: string
  theirSequence: number
  ourSequence: number
  lastSeen: number
}

export interface NewPeerEntry {
  nodeId: string
  url: string
}

export type Model = Record<string, PeerEntry>

export const peerTableStore = createStore<Model>("peer-table", {})

export const addPeer = (peerEntry: NewPeerEntry) =>
  produce<Model>((draft) => {
    draft[peerEntry.nodeId] = {
      ...peerEntry,
      theirSequence: 0,
      ourSequence: 0,
      lastSeen: Date.now(),
    }
  })
export const updatePeer = (nodeId: string, peerEntry: Partial<PeerEntry>) =>
  produce<Model>((draft) => {
    const previousEntry = draft[nodeId]
    if (previousEntry == null) {
      throw new Error("nodeId not found")
    }
    draft[nodeId] = {
      ...previousEntry,
      ...peerEntry,
    }
  })
