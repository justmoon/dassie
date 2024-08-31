import { uint8ArrayToBase64 } from "uint8array-extras"

import type { ServerState } from "./state"

const ID_LENGTH = 4

export function generateUniqueId(
  state: Pick<
    ServerState,
    "context" | "activeCredentials" | "activeConnections"
  >,
) {
  for (;;) {
    const uniqueId = uint8ArrayToBase64(
      state.context.crypto.getRandomBytes(ID_LENGTH),
      { urlSafe: true },
    )

    if (state.activeCredentials.has(uniqueId)) {
      continue
    }
    if (state.activeConnections.has(uniqueId)) {
      continue
    }

    return uniqueId
  }
}
