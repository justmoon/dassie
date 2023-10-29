import type { PeerMessage } from "../peer-schema"

export const ALLOW_ANONYMOUS_USAGE: string[] = [
  "peeringRequest",
  "linkStateRequest",
  "nodeListHashRequest",
  "nodeListRequest",
  "registration",
] satisfies PeerMessage["content"]["value"]["type"][]
