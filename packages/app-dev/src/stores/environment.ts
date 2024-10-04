import type { SettlementSchemeId } from "@dassie/app-dassie/src/peer-protocol/types/settlement-scheme-id"
import { createStore } from "@dassie/lib-reactive"

export interface EnvironmentSettings {
  readonly additionalNodeStartIndex: number
  readonly defaultNodeSettings: NodeSettings
  readonly bootstrapNodes: readonly number[]
  readonly registeredNodes: readonly number[]
}

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

export const DEFAULT_ENVIRONMENT: EnvironmentSettings = {
  additionalNodeStartIndex: 0,
  defaultNodeSettings: {},
  bootstrapNodes: [0, 1],
  registeredNodes: [0, 1],
}

export const EnvironmentStore = () => {
  const initialEnvironment: EnvironmentSettings = DEFAULT_ENVIRONMENT

  return createStore(initialEnvironment).actions({
    setEnvironment: (environment: EnvironmentSettings) => () => environment,
  })
}
