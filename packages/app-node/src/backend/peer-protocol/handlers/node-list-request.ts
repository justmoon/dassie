import { Reactor } from "@dassie/lib-reactive"

import { PeerMessageHandler } from "../actors/handle-peer-message"
import { SerializedNodeListSignal } from "../computed/serialized-node-list"

export const HandleNodeListRequest = ((reactor: Reactor) => {
  const serializedNodeListSignal = reactor.use(SerializedNodeListSignal)

  return () => ({ bytes: serializedNodeListSignal.read() })
}) satisfies PeerMessageHandler<"nodeListRequest">
