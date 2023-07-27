import { createComputed } from "@dassie/lib-reactive"
import { UnreachableCaseError } from "@dassie/lib-type-utils"

import { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"
import { settlementSchemesStore } from "../database-stores/settlement-schemes"

export const activeSettlementSchemesSignal = () =>
  createComputed((sig) => {
    const settlementSchemes = sig.use(settlementSchemesStore)

    const activeSettlementSchemes = new Set<SettlementSchemeId>()

    for (const settlementScheme of settlementSchemes.read()) {
      activeSettlementSchemes.add(settlementScheme.id)
    }

    settlementSchemes.changes.on(sig.reactor, (change) => {
      switch (change[0]) {
        case "addSettlementScheme": {
          const [settlementSchemeId] = change[1]

          activeSettlementSchemes.add(settlementSchemeId)
          break
        }
        default: {
          throw new UnreachableCaseError(change[0])
        }
      }
    })

    return activeSettlementSchemes
  })
