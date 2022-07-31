import { sign } from "@xen-ilp/lib-crypto"
import type { EffectContext } from "@xen-ilp/lib-reactive"

import { configStore } from "../config"
import { parseEd25519PrivateKey } from "../utils/pem"
import { encodeMessage } from "./codecs/xen-message"
import {
  outgoingUnsignedXenMessageTopic,
  outgoingXenMessageBufferTopic,
} from "./topics/xen-protocol"

export const outgoingXenMessageSigner = (sig: EffectContext) => {
  const xenKey = parseEd25519PrivateKey(
    sig.get(configStore, (config) => config.tlsXenKey)
  )

  const signWithXenKey = (data: Uint8Array) => {
    return sign(xenKey, data)
  }

  sig.on(
    outgoingUnsignedXenMessageTopic,
    ({ envelope: message, destination }) => {
      sig.emit(outgoingXenMessageBufferTopic, {
        envelope: encodeMessage(message, signWithXenKey),
        destination,
      })
    }
  )
}