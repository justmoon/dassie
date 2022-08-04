import { createTopic } from "@xen-ilp/lib-reactive"

export interface PeerMessageMetadata {
  from: string
  to: string
}

export const peerTrafficTopic = () => createTopic<PeerMessageMetadata>()
