import { sign } from "@xen-ilp/lib-crypto"
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
        return sign(xenKey, data)
      },
    }
  })
