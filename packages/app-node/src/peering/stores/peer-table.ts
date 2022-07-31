import produce, { enableMapSet } from "immer"

import { createStore } from "@xen-ilp/lib-reactive"

enableMapSet()

export interface PeerEntry {
  nodeId: string
  url: string
  sequence: bigint
  lastSeen: number
}

export interface NewPeerEntry {
  nodeId: string
  url: string
  sequence?: bigint
  lastSeen?: number
}

export type Model = Map<string, PeerEntry>

export const peerTableStore = () => createStore<Model>(new Map())

export const addPeer = (peerEntry: NewPeerEntry) =>
  produce<Model>((draft) => {
    draft.set(peerEntry.nodeId, {
      sequence: 0n,
      lastSeen: Date.now(),
      ...peerEntry,
    })
  })

export const updatePeer = (nodeId: string, peerEntry: Partial<PeerEntry>) =>
  produce<Model>((draft) => {
    const previousEntry = draft.get(nodeId)
    if (previousEntry == undefined) {
      throw new Error("nodeId not found")
    }
    draft.set(nodeId, {
      ...previousEntry,
      ...peerEntry,
    })
  })
