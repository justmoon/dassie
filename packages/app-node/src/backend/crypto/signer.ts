import { signAsync } from "@noble/ed25519"

import { createActor } from "@dassie/lib-reactive"

import { NodePrivateKeySignal } from "./computed/node-private-key"

export const SignerActor = () =>
  createActor((sig) => {
    const dassieKey = sig.get(NodePrivateKeySignal)

    return sig.handlers({
      signWithDassieKey(data: Uint8Array) {
        return signAsync(data, dassieKey)
      },
    })
  })
