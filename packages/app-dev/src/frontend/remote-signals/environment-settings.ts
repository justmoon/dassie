import type { EnvironmentSettings } from "../../backend/stores/environment-settings"
import { createRemoteSignal } from "../utils/remote-reactive"

export const environmentSettingsStore = () =>
  createRemoteSignal("environmentSettings", {} as EnvironmentSettings)
