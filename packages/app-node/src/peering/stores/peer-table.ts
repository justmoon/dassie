import produce from "immer"

import { createStore } from "@xen-ilp/lib-reactive"

export interface PeerEntry {
  nodeId: string
  url: string
  theirSequence: number
  lastSeen: number
}

export interface NewPeerEntry {
  nodeId: string
  url: string
  theirSequence?: number
  lastSeen?: number
}

export type Model = Record<string, PeerEntry>

export const peerTableStore = () => createStore<Model>({})

export const addPeer = (peerEntry: NewPeerEntry) =>
  produce<Model>((draft) => {
    draft[peerEntry.nodeId] = {
      theirSequence: 0,
      lastSeen: Date.now(),
      ...peerEntry,
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
