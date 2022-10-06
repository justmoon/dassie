import { getPublicKey, sign } from "@noble/ed25519"

import { createService } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { parseEd25519PrivateKey } from "../utils/pem"

export const signerService = () =>
  createService((sig) => {
    const dassieKey = parseEd25519PrivateKey(
      sig.get(configSignal, (config) => config.tlsDassieKey)
    )

    return {
      getPublicKey(): Promise<Uint8Array> {
        return getPublicKey(dassieKey)
      },
      signWithDassieKey(data: Uint8Array) {
        return sign(data, dassieKey)
      },
    }
  })
