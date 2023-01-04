import produce, { enableMapSet } from "immer"
import type { MarkOptional } from "ts-essentials"

import { createStore } from "@dassie/lib-reactive"

import type { NodeTableKey } from "./node-table"

enableMapSet()

export type PeerState =
  | {
      id: "request-peering"
    }
  | {
      id: "peered"
    }

export interface PeerEntry {
  subnetId: string
  nodeId: string
  state: PeerState
  url: string
  nodePublicKey: Uint8Array
  lastSeen: number
}

export type NewPeerEntry = MarkOptional<PeerEntry, "lastSeen">

export const peerTableStore = () =>
  createStore(new Map<NodeTableKey, PeerEntry>(), {
    addPeer: (peerEntry: NewPeerEntry) =>
      produce((draft) => {
        draft.set(`${peerEntry.subnetId}.${peerEntry.nodeId}`, {
          lastSeen: Date.now(),
          ...peerEntry,
        })
      }),
    updatePeer: (key: NodeTableKey, peerEntry: Partial<PeerEntry>) =>
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
