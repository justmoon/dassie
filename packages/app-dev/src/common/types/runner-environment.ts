import { EnvironmentVariables } from "@dassie/app-node/src/backend/config/types/environment-variables"

export interface RunnerEnvironment extends EnvironmentVariables {
  readonly DASSIE_DEV_ROOT?: string
  readonly DASSIE_DEV_BASE?: string
  readonly DASSIE_DEV_ENTRY?: string
  readonly DASSIE_DEV_RPC_URL?: string
  readonly DASSIE_DEV_NODE_ID?: string
  readonly DASSIE_DEV_DEBUG_RPC_PORT?: string
}
