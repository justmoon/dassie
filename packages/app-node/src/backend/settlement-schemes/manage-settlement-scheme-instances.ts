import { Reactor, createActor, createMapped } from "@dassie/lib-reactive"

import { initializeCommonAccounts } from "../accounting/functions/manage-common-accounts"
import { LedgerStore } from "../accounting/stores/ledger"
import { DatabaseConfigStore } from "../config/database-config"
import { SendPeerMessageActor } from "../peer-protocol/actors/send-peer-message"
import modules from "./modules"
import { ActiveSettlementSchemesSignal } from "./signals/active-settlement-schemes"
import type { SettlementSchemeHostMethods } from "./types/settlement-scheme-module"
import { getLedgerIdForSettlementScheme } from "./utils/get-ledger-id"

export const ManageSettlementSchemeInstancesActor = (reactor: Reactor) => {
  const ledgerStore = reactor.use(LedgerStore)

  return createMapped(
    reactor,
    ActiveSettlementSchemesSignal,
    (settlementSchemeId) => {
      const { realm } = reactor.use(DatabaseConfigStore).read()

      const module = modules[settlementSchemeId]
      if (!module) {
        throw new Error(`Unknown settlement scheme '${settlementSchemeId}'`)
      }

      if (realm !== module.realm) {
        throw new Error("Subnet module is not compatible with realm")
      }

      const ledgerId = getLedgerIdForSettlementScheme(settlementSchemeId)

      initializeCommonAccounts(ledgerStore, ledgerId)

      const host: SettlementSchemeHostMethods = {
        sendMessage: ({ peerId, message }) => {
          reactor.use(SendPeerMessageActor).api.send.tell({
            destination: peerId,
            message: {
              type: "settlementMessage",
              value: {
                settlementSchemeId,
                message,
              },
            },
          })
        },
      }

      // Instantiate settlement scheme module actor.
      return createActor((sig) =>
        module.behavior({ sig, settlementSchemeId, host }),
      )
    },
  )
}
