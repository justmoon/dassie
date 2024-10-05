import { mkdirSync, readFileSync, writeFileSync } from "node:fs"
import path from "node:path"

import { createActor } from "@dassie/lib-reactive"

import { DEV_SERVER_STATE_PATH } from "../constants/paths"
import { scenarios } from "../scenarios"
import { ScenarioSignal } from "../signals/scenario"

const SCENARIO_FILE_PATH = path.resolve(DEV_SERVER_STATE_PATH, "scenario.txt")

function validateScenario(
  scenario: string,
): scenario is keyof typeof scenarios {
  return scenario in scenarios
}

export const PersistScenarioActor = () =>
  createActor((sig) => {
    const scenarioSignal = sig.reactor.use(ScenarioSignal)

    try {
      const lastScenario = readFileSync(SCENARIO_FILE_PATH, "utf8")
      if (validateScenario(lastScenario)) {
        scenarioSignal.write(lastScenario)
      }
    } catch {
      // ignore errors
    }

    sig.reactor.use(ScenarioSignal).values.on(sig, (scenarioId) => {
      mkdirSync(DEV_SERVER_STATE_PATH, { recursive: true })
      writeFileSync(SCENARIO_FILE_PATH, scenarioId, "utf8")
    })
  })
