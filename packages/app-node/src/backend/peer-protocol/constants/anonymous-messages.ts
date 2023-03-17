import type { PeerMessage } from "../peer-schema"

export const ALLOW_ANONYMOUS_USAGE: string[] = [
  "peeringRequest",
  "linkStateRequest",
] satisfies (keyof PeerMessage["content"]["value"])[]
