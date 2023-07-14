import { createComputed } from "@dassie/lib-reactive"

import { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"
import { settlementSchemesStore } from "../database-stores/settlement-schemes"

export const activeSettlementSchemesSignal = () =>
  createComputed((sig) => {
    const settlementSchemes = sig.use(settlementSchemesStore).read()

    const activeSettlementSchemes = new Set<SettlementSchemeId>()

    for (const settlementScheme of settlementSchemes) {
      activeSettlementSchemes.add(settlementScheme.id)
    }

    return activeSettlementSchemes
  })
