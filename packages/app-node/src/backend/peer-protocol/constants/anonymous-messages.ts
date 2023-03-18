import type { PeerMessage } from "../peer-schema"

export const ALLOW_ANONYMOUS_USAGE: string[] = [
  "peeringRequest",
  "linkStateRequest",
] satisfies PeerMessage["content"]["value"]["type"][]
