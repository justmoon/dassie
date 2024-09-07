import { generateUniqueId } from "./generate-unique-id"
import type { ServerState } from "./state"

const SHARED_SECRET_LENGTH = 32

export interface StreamCredentials {
  destination: string
  secret: Uint8Array
}

export function generateCredentials(
  state: Pick<
    ServerState,
    "context" | "configuration" | "activeCredentials" | "activeConnections"
  >,
): StreamCredentials {
  const uniqueId = generateUniqueId(state)
  const secret = state.context.crypto.getRandomBytes(SHARED_SECRET_LENGTH)

  state.activeCredentials.set(uniqueId, secret)

  return { destination: `${state.configuration.address}.${uniqueId}`, secret }
}
