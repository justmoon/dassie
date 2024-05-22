import type { Factory, LifecycleScope } from "@dassie/lib-reactive"

export interface StartScenarioParameters {
  lifecycle: LifecycleScope
}

export interface Scenario {
  name: string
  description: string
  StartScenario: Factory<(parameters: StartScenarioParameters) => Promise<void>>
}
