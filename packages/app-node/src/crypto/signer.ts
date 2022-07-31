import { sign } from "@noble/ed25519"

import { createValue } from "@xen-ilp/lib-reactive"

import { configStore } from "../config"
import { parseEd25519PrivateKey } from "../utils/pem"

export const signerValue = () =>
  createValue((sig) => {
    const xenKey = parseEd25519PrivateKey(
      sig.get(configStore, (config) => config.tlsXenKey)
    )

    return {
      signWithXenKey(data: Uint8Array) {
        return sign(data, xenKey)
      },
    }
  })
