import type {
  AbortContext,
  Factory,
  LifecycleContext,
} from "@dassie/lib-reactive"

export interface StartScenarioParameters {
  context: LifecycleContext & AbortContext
}

export interface Scenario {
  name: string
  description: string
  StartScenario: Factory<(parameters: StartScenarioParameters) => Promise<void>>
}
