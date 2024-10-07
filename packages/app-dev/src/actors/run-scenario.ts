import { createActor } from "@dassie/lib-reactive"

import { children as logger } from "../logger/instances"
import { scenarios } from "../scenarios"
import { ScenarioSignal } from "../signals/scenario"
import { EnvironmentStore } from "../stores/environment"
import { PeeringStateStore } from "../stores/peering-state"
import type {
  DevelopmentActorContext,
  DevelopmentReactor,
} from "../types/development-base"
import { RunAdditionalNodesActor } from "./run-additional-nodes"

export const RunScenarioActor = (reactor: DevelopmentReactor) => {
  return createActor(async (sig: DevelopmentActorContext) => {
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
