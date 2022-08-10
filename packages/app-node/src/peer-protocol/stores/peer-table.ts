import { createStore } from "@dassie/lib-reactive"
import produce, { enableMapSet } from "immer"
import type { MarkOptional } from "ts-essentials"

enableMapSet()

export interface PeerEntry {
  nodeId: string
  url: string
  lastSeen: number
}

export type NewPeerEntry = MarkOptional<PeerEntry, "lastSeen">

export type Model = Map<string, PeerEntry>

export const peerTableStore = () => createStore<Model>(new Map())

export const addPeer = (peerEntry: NewPeerEntry) =>
  produce<Model>((draft) => {
    draft.set(peerEntry.nodeId, {
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
