import { enableMapSet, produce } from "immer"

import { createStore } from "@dassie/lib-reactive"

import type { NodeId } from "../types/node-id"

enableMapSet()

export const SESSION_KEY_EXPIRY = 1000 * 60 * 60 * 24 * 7 // 7 days

export interface OutgoingSessionKeyEntry {
  sessionPublicKey: Uint8Array
  sharedSecret: Uint8Array
  createdAt: Date
}
export const OutgoingSessionKeysStore = () =>
  createStore(new Map<NodeId, OutgoingSessionKeyEntry>()).actions({
    addKeyEntry: (destination: NodeId, entry: OutgoingSessionKeyEntry) =>
      produce((draft) => {
        draft.set(destination, entry)
      }),
  })
