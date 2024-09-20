import { enableMapSet, produce } from "immer"

import { type Reactor, createComputed } from "@dassie/lib-reactive"
import { UnreachableCaseError } from "@dassie/lib-type-utils"

import type { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"
import { SettlementSchemesStore } from "../database-stores/settlement-schemes"

enableMapSet()

export const ActiveSettlementSchemesSignal = (reactor: Reactor) =>
  createComputed(reactor, () => {
    const settlementSchemes = reactor.use(SettlementSchemesStore)

    const activeSettlementSchemes = new Set<SettlementSchemeId>()

    for (const settlementScheme of settlementSchemes.read()) {
      activeSettlementSchemes.add(settlementScheme.id)
    }

    settlementSchemes.changes.on(reactor, (change) => {
      switch (change[0]) {
        case "addSettlementScheme": {
          const [settlementSchemeId] = change[1]

          const newSet = produce(activeSettlementSchemes, (draft) => {
            draft.add(settlementSchemeId)
          })
          reactor.use(ActiveSettlementSchemesSignal).write(newSet)
          break
        }
        default: {
          throw new UnreachableCaseError(change[0])
        }
      }
    })

    return activeSettlementSchemes
  })
