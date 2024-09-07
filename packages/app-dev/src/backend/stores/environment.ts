import type { SettlementSchemeId } from "@dassie/app-node/src/backend/peer-protocol/types/settlement-scheme-id"
import { createStore } from "@dassie/lib-reactive"

export interface EnvironmentSettings {
  readonly additionalNodeStartIndex?: number | undefined
  readonly defaultNodeSettings?: NodeSettings | undefined
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

export const DEFAULT_ADDITIONAL_NODE_START_INDEX = 10

export const EnvironmentStore = () => {
  const initialEnvironment: EnvironmentSettings = {}

  return createStore(initialEnvironment).actions({
    setEnvironment: (environment: EnvironmentSettings) => () => environment,
  })
}
