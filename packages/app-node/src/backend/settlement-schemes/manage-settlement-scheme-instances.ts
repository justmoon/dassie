import { createActor, createMapped } from "@dassie/lib-reactive"
import { isFailure, tell } from "@dassie/lib-type-utils"

import { initializeCommonAccounts } from "../accounting/functions/manage-common-accounts"
import { LedgerStore } from "../accounting/stores/ledger"
import {
  AssetsInterledgerPeerAccount,
  AssetsOnLedgerAccount,
  EquityOwnerAccount,
} from "../accounting/types/account-paths"
import { DassieReactor } from "../base/types/dassie-base"
import { DatabaseConfigStore } from "../config/database-config"
import { settlement as logger } from "../logger/instances"
import { SendPeerMessage } from "../peer-protocol/functions/send-peer-message"
import { getLedgerIdForSettlementScheme } from "./functions/get-ledger-id"
import modules from "./modules"
import { ActiveSettlementSchemesSignal } from "./signals/active-settlement-schemes"
import type { SettlementSchemeHostMethods } from "./types/settlement-scheme-module"
import { PendingSettlementsMap } from "./values/pending-settlements-map"

export const ManageSettlementSchemeInstancesActor = (
  reactor: DassieReactor,
) => {
  const ledgerStore = reactor.use(LedgerStore)
  const pendingSettlementsMap = reactor.use(PendingSettlementsMap)
  const sendPeerMessage = reactor.use(SendPeerMessage)

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
          tell(() =>
            sendPeerMessage({
              destination: peerId,
              message: {
                type: "settlementMessage",
                value: {
                  settlementSchemeId,
                  message,
                },
              },
            }),
          )
        },

        reportIncomingSettlement: ({ ledgerId, peerId, amount }) => {
          logger.debug("reported incoming settlement", {
            settlementSchemeId,
            ledgerId,
            peerId,
            amount,
          })

          const onLedgerPath: AssetsOnLedgerAccount = `${ledgerId}:assets/settlement`
          const peerPath: AssetsInterledgerPeerAccount = `${ledgerId}:assets/interledger/${peerId}`

          const result = ledgerStore.createTransfer({
            debitAccountPath: onLedgerPath,
            creditAccountPath: peerPath,
            amount,
          })

          if (isFailure(result)) {
            throw new Error(
              `Could not credit settlement in internal ledger: ${result.name}`,
            )
          }
        },

        finalizeOutgoingSettlement: ({ settlementId }) => {
          logger.debug("finalizing outgoing settlement", {
            settlementSchemeId,
            ledgerId,
            settlementId,
          })

          const settlementKey = `${settlementSchemeId}:${settlementId}`
          const transfer = pendingSettlementsMap.get(settlementKey)

          if (!transfer) {
            throw new Error(
              `Could not finalize outgoing settlement: transfer not found`,
            )
          }

          ledgerStore.postPendingTransfer(transfer)
          pendingSettlementsMap.delete(settlementKey)
        },

        cancelOutgoingSettlement: ({ settlementId }) => {
          logger.debug("canceling outgoing settlement", {
            settlementSchemeId,
            ledgerId,
            settlementId,
          })

          const settlementKey = `${settlementSchemeId}:${settlementId}`
          const transfer = pendingSettlementsMap.get(settlementKey)

          if (!transfer) {
            throw new Error(
              `Could not cancel outgoing settlement: transfer not found`,
            )
          }

          ledgerStore.voidPendingTransfer(transfer)
          pendingSettlementsMap.delete(settlementKey)
        },

        reportDeposit: ({ ledgerId, amount }) => {
          logger.debug("reported deposit", {
            settlementSchemeId,
            ledgerId,
            amount,
          })

          const onLedgerPath: AssetsOnLedgerAccount = `${ledgerId}:assets/settlement`
          const ownerEquityPath: EquityOwnerAccount = `${ledgerId}:equity/owner`

          const result = ledgerStore.createTransfer({
            debitAccountPath: onLedgerPath,
            creditAccountPath: ownerEquityPath,
            amount,
          })

          if (isFailure(result)) {
            throw new Error(
              `Could not credit deposit in internal ledger: ${result.name}`,
            )
          }
        },

        reportWithdrawal: ({ ledgerId, amount }) => {
          logger.debug("reported withdrawal", {
            settlementSchemeId,
            ledgerId,
            amount,
          })

          const onLedgerPath: AssetsOnLedgerAccount = `${ledgerId}:assets/settlement`
          const ownerEquityPath: EquityOwnerAccount = `${ledgerId}:equity/owner`

          const result = ledgerStore.createTransfer({
            debitAccountPath: ownerEquityPath,
            creditAccountPath: onLedgerPath,
            amount,
          })

          if (isFailure(result)) {
            throw new Error(
              `Could not credit deposit in internal ledger: ${result.name}`,
            )
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
