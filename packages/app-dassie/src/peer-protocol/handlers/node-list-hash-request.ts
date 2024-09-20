import type { Reactor } from "@dassie/lib-reactive"

import { NodeListHashSignal } from "../computed/node-list-hash"
import type { PeerMessageHandler } from "../functions/handle-peer-message"

export const HandleNodeListHashRequest = ((reactor: Reactor) => {
  const nodeListHashSignal = reactor.use(NodeListHashSignal)

  return () => ({ hash: nodeListHashSignal.read() })
}) satisfies PeerMessageHandler<"nodeListHashRequest">
