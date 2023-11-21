import { Reactor } from "@dassie/lib-reactive"
import { UnreachableCaseError, isFailure } from "@dassie/lib-type-utils"

import { processSettlementPrepare } from "../../accounting/functions/process-settlement"
import { LedgerStore } from "../../accounting/stores/ledger"
import { ManageSettlementSchemeInstancesActor } from "../../settlement-schemes/manage-settlement-scheme-instances"
import { getLedgerIdForSettlementScheme } from "../../settlement-schemes/utils/get-ledger-id"
import type { PeerMessageHandler } from "../actors/handle-peer-message"
import { NodeTableStore } from "../stores/node-table"

export const HandleSettlement = ((reactor: Reactor) => {
  const settlementSchemeManager = reactor.use(
    ManageSettlementSchemeInstancesActor,
  )
  const ledgerStore = reactor.use(LedgerStore)
  const nodeTableStore = reactor.use(NodeTableStore)

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

    const ledgerId = getLedgerIdForSettlementScheme(settlementSchemeId)

    const settlementTransfer = processSettlementPrepare(
      ledgerStore,
      ledgerId,
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

    const peerState = nodeTableStore.read().get(sender)?.peerState

    if (peerState?.id !== "peered") {
      throw new Error(`Settlement failed, peer ${sender} is not peered`)
    }

    const { result } = await settlementSchemeActor.api.handleSettlement.ask({
      peerId: sender,
      amount,
      proof,
      peerState: peerState.settlementSchemeState,
    })

    if (result === "accept") {
      ledgerStore.postPendingTransfer(settlementTransfer)
    } else {
      ledgerStore.voidPendingTransfer(settlementTransfer)
    }
  }
}) satisfies PeerMessageHandler<"settlement">
