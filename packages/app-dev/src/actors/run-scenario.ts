import { type Reactor, createActor } from "@dassie/lib-reactive"

import { children as logger } from "../logger/instances"
import { scenarios } from "../scenarios"
import { ScenarioSignal } from "../signals/scenario"
import { EnvironmentStore } from "../stores/environment"
import { PeeringStateStore } from "../stores/peering-state"
import { RunAdditionalNodesActor } from "./run-additional-nodes"

export const RunScenarioActor = (reactor: Reactor) => {
  return createActor(async (sig) => {
    const scenarioId = sig.readAndTrack(ScenarioSignal)
    try {
      reactor.use(PeeringStateStore).act.clear()
      reactor.use(EnvironmentStore).act.resetEnvironment()
      await reactor.use(scenarios[scenarioId].StartScenario)({ context: sig })
    } catch (error) {
      logger.error("error starting scenario", { error, scenario: scenarioId })
    }

    await sig.run(RunAdditionalNodesActor)
  })
}
