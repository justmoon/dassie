import { createTopic } from "@dassie/lib-reactive"

export interface PeerMessageMetadata {
  from: string
  to: string
}

export const PeerTrafficTopic = () => createTopic<PeerMessageMetadata>()
