import { type Reactor, createComputed } from "@dassie/lib-reactive"

import { ActiveNodesStore } from "../stores/active-nodes"
import { nodeIndexToFriendlyId } from "../utils/generate-node-config"

export const ActiveNodeIdsComputed = (reactor: Reactor) =>
  createComputed(reactor, (sig) =>
    sig.readAndTrack(ActiveNodesStore, (activeNodes) =>
      [...activeNodes].map(({ index }) => nodeIndexToFriendlyId(index)),
    ),
  )
