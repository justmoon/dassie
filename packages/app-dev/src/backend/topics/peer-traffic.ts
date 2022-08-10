import { createTopic } from "@dassie/lib-reactive"

export interface PeerMessageMetadata {
  from: string
  to: string
}

export const peerTrafficTopic = () => createTopic<PeerMessageMetadata>()
