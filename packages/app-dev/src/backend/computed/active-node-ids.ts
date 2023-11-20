import { Reactor, createComputed } from "@dassie/lib-reactive"

import { nodeIndexToFriendlyId } from "../utils/generate-node-config"
import { ActiveNodesComputed } from "./active-nodes"

export const ActiveNodeIdsComputed = (reactor: Reactor) =>
  createComputed(reactor, (sig) =>
    sig.readAndTrack(ActiveNodesComputed, (activeNodes) =>
      [...activeNodes].map((index) => nodeIndexToFriendlyId(index)),
    ),
  )
