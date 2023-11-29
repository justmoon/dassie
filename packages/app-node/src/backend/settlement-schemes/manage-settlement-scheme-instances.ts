import { Reactor, createActor, createMapped } from "@dassie/lib-reactive"
import { isFailure } from "@dassie/lib-type-utils"

import { initializeCommonAccounts } from "../accounting/functions/manage-common-accounts"
import { LedgerStore } from "../accounting/stores/ledger"
import {
  AssetsOnLedgerAccount,
  EquitySuspenseAccount,
} from "../accounting/types/account-paths"
import { DatabaseConfigStore } from "../config/database-config"
import { settlement as logger } from "../logger/instances"
import { SendPeerMessageActor } from "../peer-protocol/actors/send-peer-message"
import { GetLedgerIdForSettlementScheme } from "./functions/get-ledger-id"
import modules from "./modules"
import { ActiveSettlementSchemesSignal } from "./signals/active-settlement-schemes"
import type { SettlementSchemeHostMethods } from "./types/settlement-scheme-module"

export const ManageSettlementSchemeInstancesActor = (reactor: Reactor) => {
  const ledgerStore = reactor.use(LedgerStore)
  const getLedgerIdForSettlementScheme = reactor.use(
    GetLedgerIdForSettlementScheme,
  )

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

        reportOnLedgerBalance: ({ ledgerId, balance }) => {
          const onLedgerPath: AssetsOnLedgerAccount = `${ledgerId}:assets/settlement`
          const suspensePath: EquitySuspenseAccount = `${ledgerId}:equity/suspense`

          const internalAccount = ledgerStore.getAccount(onLedgerPath)
          logger.assert(
            !!internalAccount,
            `internal account '${onLedgerPath}' not found`,
          )

          const internalBalance =
            internalAccount.debitsPosted -
            internalAccount.creditsPosted -
            internalAccount.creditsPending

          if (internalBalance !== balance) {
            const result = ledgerStore.createTransfer({
              debitAccountPath: onLedgerPath,
              creditAccountPath: suspensePath,
              amount: balance - internalBalance,
            })

            if (isFailure(result)) {
              throw new Error(
                `Could not create suspense transfer: ${result.name}`,
              )
            }
          }
        },
      }

      // Instantiate settlement scheme module actor.
      return createActor((sig) =>
        module.behavior({ sig, settlementSchemeId, host }),
      )
    },
  )
}
