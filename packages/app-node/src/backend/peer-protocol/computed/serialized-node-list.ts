import { Reactor, createComputed } from "@dassie/lib-reactive"

import { peerMessageResponse } from "../peer-schema"
import { NodeSetSignal } from "./node-set"

export const SerializedNodeListSignal = (reactor: Reactor) =>
  createComputed(reactor.lifecycle, (sig) => {
    const nodeSet = sig.get(reactor.use(NodeSetSignal))

    return peerMessageResponse.nodeListRequest.serializeOrThrow({
      value: [...nodeSet],
    })
  })
