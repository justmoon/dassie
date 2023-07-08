import { createSignal } from "@dassie/lib-reactive"

import { NodeId } from "../../peer-protocol/types/node-id"
import { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"

export interface SettlementSchemeState {
  settlementSchemeId: SettlementSchemeId
  config: unknown
  initialPeers: readonly {
    nodeId: NodeId
    url: string
    alias?: string | undefined
    nodePublicKey: string
  }[]
}

export const settlementSchemeMapSignal = () =>
  createSignal<Map<string, SettlementSchemeState>>(new Map())
