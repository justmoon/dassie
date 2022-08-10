import { getPublicKey, sign } from "@noble/ed25519"

import { createValue } from "@dassie/lib-reactive"

import { configStore } from "../config"
import { parseEd25519PrivateKey } from "../utils/pem"

export const signerValue = () =>
  createValue((sig) => {
    const dassieKey = parseEd25519PrivateKey(
      sig.get(configStore, (config) => config.tlsDassieKey)
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
