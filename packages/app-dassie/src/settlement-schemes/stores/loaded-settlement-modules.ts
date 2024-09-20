import { produce } from "immer"

import { createStore } from "@dassie/lib-reactive"

import type { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"
import type { SettlementSchemeModule } from "../types/settlement-scheme-module"

export const LoadedSettlementModulesStore = () =>
  createStore(new Map<string, SettlementSchemeModule>()).actions({
    loadModule: (
      settlementSchemeId: SettlementSchemeId,
      module: SettlementSchemeModule,
    ) =>
      produce((state) => {
        state.set(settlementSchemeId, module)
      }),
  })
