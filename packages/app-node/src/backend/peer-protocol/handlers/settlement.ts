import { Reactor } from "@dassie/lib-reactive"
import { UnreachableCaseError, isFailure } from "@dassie/lib-type-utils"

import { processSettlementPrepare } from "../../accounting/functions/process-settlement"
import { LedgerStore } from "../../accounting/stores/ledger"
import { ManageSettlementSchemeInstancesActor } from "../../settlement-schemes/manage-settlement-scheme-instances"
import type { PeerMessageHandler } from "../actors/handle-peer-message"

export const HandleSettlement = ((reactor: Reactor) => {
  const settlementSchemeManager = reactor.use(
    ManageSettlementSchemeInstancesActor,
  )
  const ledgerStore = reactor.use(LedgerStore)

  return async ({
    message: {
      sender,
      content: {
        value: {
          value: { settlementSchemeId, amount, proof },
        },
      },
    },
  }) => {
    const settlementSchemeActor =
      settlementSchemeManager.get(settlementSchemeId)

    if (!settlementSchemeActor) return

    const settlementTransfer = processSettlementPrepare(
      ledgerStore,
      settlementSchemeId,
      sender,
      amount,
      "incoming",
    )

    if (isFailure(settlementTransfer)) {
      switch (settlementTransfer.name) {
        case "InvalidAccountFailure": {
          throw new Error(
            `Settlement failed, invalid ${settlementTransfer.whichAccount} account ${settlementTransfer.accountPath}`,
          )
        }

        case "ExceedsCreditsFailure": {
          throw new Error(`Settlement failed, exceeds credits`)
        }

        case "ExceedsDebitsFailure": {
          throw new Error(`Settlement failed, exceeds debits`)
        }

        default: {
          throw new UnreachableCaseError(settlementTransfer)
        }
      }
    }

    const { result } = await settlementSchemeActor.api.handleSettlement.ask({
      peerId: sender,
      amount,
      proof,
    })

    if (result === "accept") {
      ledgerStore.postPendingTransfer(settlementTransfer)
    } else {
      ledgerStore.voidPendingTransfer(settlementTransfer)
    }
  }
}) satisfies PeerMessageHandler<"settlement">
