import { signAsync } from "@noble/ed25519"

import { createActor } from "@dassie/lib-reactive"

import { nodePrivateKeySignal } from "./computed/node-private-key"

export const signerService = () =>
  createActor((sig) => {
    const dassieKey = sig.get(nodePrivateKeySignal)

    return {
      signWithDassieKey(data: Uint8Array) {
        return signAsync(data, dassieKey)
      },
    }
  })
