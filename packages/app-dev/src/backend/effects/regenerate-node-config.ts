import type { EffectContext } from "@dassie/lib-reactive"

import { activeNodesStore } from "../stores/active-nodes"
import {
  DEFAULT_ENVIRONMENT_SETTINGS,
  environmentSettingsStore,
} from "../stores/environment-settings"

export const regenerateNodeConfig = (sig: EffectContext) => {
  const environmentSettings = sig.get(environmentSettingsStore)

  if (environmentSettings === DEFAULT_ENVIRONMENT_SETTINGS) return

  sig.use(activeNodesStore).regenerateConfig(environmentSettings)
}
