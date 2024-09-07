import { type Reactor, createComputed } from "@dassie/lib-reactive"

import { peerMessageResponse } from "../peer-schema"
import { NodeSetSignal } from "./node-set"

export const SerializedNodeListSignal = (reactor: Reactor) =>
  createComputed(reactor, (sig) => {
    const nodeSet = sig.readAndTrack(NodeSetSignal)

    return peerMessageResponse.nodeListRequest.serializeOrThrow({
      value: [...nodeSet],
    })
  })
