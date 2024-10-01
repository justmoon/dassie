import { enableMapSet, produce } from "immer"

import {
  type InferActionHandlers,
  type Reactor,
  createComputed,
} from "@dassie/lib-reactive"

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

    settlementSchemes.changes.on(reactor, ([actionId, parameters]) => {
      const handlers: InferActionHandlers<typeof settlementSchemes> = {
        addSettlementScheme: (settlementSchemeId) => {
          const newSet = produce(activeSettlementSchemes, (draft) => {
            draft.add(settlementSchemeId)
          })
          reactor.use(ActiveSettlementSchemesSignal).write(newSet)
        },
      }

      handlers[actionId](...parameters)
    })

    return activeSettlementSchemes
  })
