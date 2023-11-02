import { enableMapSet, produce } from "immer"

import { createStore } from "@dassie/lib-reactive"

enableMapSet()

export interface IncomingSessionKeyEntry {
  sharedSecret: Uint8Array
  createdAt: Date
}

export const IncomingSessionKeysStore = () =>
  createStore(new Map<string, IncomingSessionKeyEntry>(), {
    addKeyEntry: (
      sessionPublicKeyBase64: string,
      entry: IncomingSessionKeyEntry,
    ) =>
      produce((draft) => {
        draft.set(sessionPublicKeyBase64, entry)
      }),
  })
