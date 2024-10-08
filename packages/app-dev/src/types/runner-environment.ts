import type { EnvironmentVariables } from "@dassie/app-dassie/src/config/types/environment-variables"

export interface RunnerEnvironment extends EnvironmentVariables {
  readonly DASSIE_DEV_ROOT?: string
  readonly DASSIE_DEV_BASE?: string
  readonly DASSIE_DEV_ENTRY?: string
  readonly DASSIE_DEV_RPC_URL?: string
  readonly DASSIE_DEV_NODE_ID?: string
}
