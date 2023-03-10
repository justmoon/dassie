import { createStore } from "@dassie/lib-reactive"

export const VALID_PEERING_MODES = ["autopeer", "fixed"] as const

export interface EnvironmentSettings {
  peeringMode: (typeof VALID_PEERING_MODES)[number]
}

export const DEFAULT_ENVIRONMENT_SETTINGS: EnvironmentSettings = {
  peeringMode: "autopeer",
}

export const environmentSettingsStore = () =>
  createStore(DEFAULT_ENVIRONMENT_SETTINGS, {
    setPeeringMode:
      (peeringMode: EnvironmentSettings["peeringMode"]) => () => ({
        peeringMode,
      }),
  })
