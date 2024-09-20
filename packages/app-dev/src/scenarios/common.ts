import type { AbortContext, Factory, ScopeContext } from "@dassie/lib-reactive"

export interface StartScenarioParameters {
  context: ScopeContext & AbortContext
}

export interface Scenario {
  name: string
  description: string
  StartScenario: Factory<(parameters: StartScenarioParameters) => Promise<void>>
}
