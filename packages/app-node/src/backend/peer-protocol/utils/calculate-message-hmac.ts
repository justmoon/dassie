import { createHmac } from "node:crypto"

import { bufferToUint8Array } from "@dassie/lib-type-utils"

export const calculateMessageHmac = (
  message: Uint8Array,
  sharedSecret: Uint8Array,
): Uint8Array => {
  const hmac = createHmac("sha256", sharedSecret)

  hmac.update(message)

  return bufferToUint8Array(hmac.digest())
}
