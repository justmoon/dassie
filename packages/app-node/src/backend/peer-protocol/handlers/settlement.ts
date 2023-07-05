import { createActor } from "@dassie/lib-reactive"
import { UnreachableCaseError, isFailure } from "@dassie/lib-type-utils"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import { processSettlementPrepare } from "../../accounting/functions/process-settlement"
import { ledgerStore } from "../../accounting/stores/ledger"
import { manageSubnetInstances } from "../../subnets/manage-subnet-instances"
import type { IncomingPeerMessageEvent } from "../actors/handle-peer-message"

export const handleSettlement = () =>
  createActor((sig) => {
    const subnetManager = sig.use(manageSubnetInstances)
    const ledger = sig.use(ledgerStore)

    return {
      handle: async ({
        message: {
          sender,
          content: {
            value: {
              value: { subnetId, amount, proof },
            },
          },
        },
      }: IncomingPeerMessageEvent<"settlement">) => {
        const subnetActor = subnetManager.get(subnetId)

        if (!subnetActor) return EMPTY_UINT8ARRAY

        const settlementTransfer = processSettlementPrepare(
          ledger,
          subnetId,
          sender,
          amount,
          "incoming"
        )

        if (isFailure(settlementTransfer)) {
          switch (settlementTransfer.name) {
            case "InvalidAccountFailure": {
              throw new Error(
                `Settlement failed, invalid ${settlementTransfer.whichAccount} account ${settlementTransfer.accountPath}`
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

        const { result } = await subnetActor.ask("handleSettlement", {
          peerId: sender,
          amount,
          proof,
        })

        if (result === "accept") {
          ledger.postPendingTransfer(settlementTransfer)
        } else {
          ledger.voidPendingTransfer(settlementTransfer)
        }

        return EMPTY_UINT8ARRAY
      },
    }
  })