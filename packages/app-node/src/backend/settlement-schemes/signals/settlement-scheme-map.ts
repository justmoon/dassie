import { createSignal } from "@dassie/lib-reactive"

import { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"

export interface SettlementSchemeState {
  settlementSchemeId: SettlementSchemeId
  config: unknown
}

export const settlementSchemeMapSignal = () =>
  createSignal<Map<string, SettlementSchemeState>>(new Map())
