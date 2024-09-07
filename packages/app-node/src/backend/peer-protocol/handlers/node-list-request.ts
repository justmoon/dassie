import type { Reactor } from "@dassie/lib-reactive"

import { SerializedNodeListSignal } from "../computed/serialized-node-list"
import type { PeerMessageHandler } from "../functions/handle-peer-message"

export const HandleNodeListRequest = ((reactor: Reactor) => {
  const serializedNodeListSignal = reactor.use(SerializedNodeListSignal)

  return () => ({ bytes: serializedNodeListSignal.read() })
}) satisfies PeerMessageHandler<"nodeListRequest">
