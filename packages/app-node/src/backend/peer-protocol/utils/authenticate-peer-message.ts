import type { PeerMessage } from "../peer-schema"

export const authenticatePeerMessage = (
  _peerNodeKey: Uint8Array,
  peerMessage: PeerMessage
): boolean => {
  if (peerMessage.authentication.NONE) {
    return false
  }

  // TODO: Implement authentication
  return true
}
