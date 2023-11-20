import { compareSets } from "@dassie/app-node/src/backend/utils/compare-sets"
import { Reactor, createComputed } from "@dassie/lib-reactive"

import { ScenarioStore } from "../stores/scenario"

export const ActiveNodesComputed = (reactor: Reactor) =>
  createComputed(reactor, (sig) =>
    sig.readAndTrack(
      ScenarioStore,
      (scenario) => new Set(scenario.nodes.keys()),
      compareSets,
    ),
  )
