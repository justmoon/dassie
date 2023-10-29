import { Reactor } from "@dassie/lib-reactive"

import { PeerMessageHandler } from "../actors/handle-peer-message"
import { NodeListHashSignal } from "../computed/node-list-hash"

export const HandleNodeListHashRequest = ((reactor: Reactor) => {
  const nodeListHashSignal = reactor.use(NodeListHashSignal)

  return () => ({ hash: nodeListHashSignal.read() })
}) satisfies PeerMessageHandler<"nodeListHashRequest">
