import { type Reactor, createComputed } from "@dassie/lib-reactive"

import { peerMessageResponse } from "../peer-schema"
import type { NodeId } from "../types/node-id"
import { MajorityNodeListSignal } from "./majority-node-list"
import { RegisteredNodeListSignal } from "./registered-node-list"

export const SerializedNodeListSignal = (reactor: Reactor) =>
  createComputed(reactor, (sig) => {
    const majoritySet = sig.readAndTrack(MajorityNodeListSignal)
    const registeredSet = sig.readAndTrack(RegisteredNodeListSignal)

    const combinedSet = new Set<NodeId>([...majoritySet, ...registeredSet])
    const list = [...combinedSet].sort((a, b) => a.localeCompare(b))

    return peerMessageResponse.nodeListRequest.serializeOrThrow({
      value: list,
    })
  })
