import { enableMapSet, produce } from "immer"

import {
  type Reactor,
  createComputed,
  watchStoreChanges,
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

    watchStoreChanges(reactor, settlementSchemes, {
      addSettlementScheme: (settlementSchemeId) => {
        const newSet = produce(activeSettlementSchemes, (draft) => {
          draft.add(settlementSchemeId)
        })
        reactor.use(ActiveSettlementSchemesSignal).write(newSet)
      },
      removeSettlementScheme: (settlementSchemeId) => {
        const newSet = produce(activeSettlementSchemes, (draft) => {
          draft.delete(settlementSchemeId)
        })
        reactor.use(ActiveSettlementSchemesSignal).write(newSet)
      },
    })

    return activeSettlementSchemes
  })
