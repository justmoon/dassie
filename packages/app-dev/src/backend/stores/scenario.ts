import { produce } from "immer"

import { SettlementSchemeId } from "@dassie/app-node/src/backend/peer-protocol/types/settlement-scheme-id"
import { createStore } from "@dassie/lib-reactive"

import { builtinScenarios } from "../constants/scenarios"

export interface Scenario {
  readonly environment: EnvironmentSettings
  readonly nodes: readonly NodeSettings[]
}

export interface EnvironmentSettings {}

export interface NodeSettings {
  readonly peers?: readonly PeerSettings[]
  readonly settlementMethods?: readonly string[]
}

export interface PeerSettings {
  readonly index: number
  readonly settlement: {
    readonly settlementSchemeId: SettlementSchemeId
    readonly settlementSchemeState: object
  }
}

export const ScenarioStore = () => {
  const initialScenario: Scenario = builtinScenarios["sixAutopeeringStubNodes"]
  return createStore(initialScenario, {
    setScenario: (scenario: Scenario) => () => scenario,
    addNode: () =>
      produce((draft) => {
        draft.nodes.push()
      }),
  })
}
