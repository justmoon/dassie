import { type Reactor, createActor } from "@dassie/lib-reactive"

import { children as logger } from "../logger/instances"
import { scenarios } from "../scenarios"
import { ScenarioSignal } from "../signals/scenario"
import { RunAdditionalNodesActor } from "./run-additional-nodes"

export const RunScenarioActor = (reactor: Reactor) => {
  return createActor(async (sig) => {
    const scenarioId = sig.readAndTrack(ScenarioSignal)
    try {
      await reactor.use(scenarios[scenarioId].StartScenario)(sig)
    } catch (error) {
      logger.error("error starting scenario", { error, scenario: scenarioId })
    }

    await sig.run(RunAdditionalNodesActor)
  })
}
