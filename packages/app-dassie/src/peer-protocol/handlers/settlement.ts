import type { DassieReactor } from "../../base/types/dassie-base"
import { ManageSettlementSchemeInstancesActor } from "../../ledgers/manage-settlement-scheme-instances"
import type { PeerMessageHandler } from "../functions/handle-peer-message"
import { NodeTableStore } from "../stores/node-table"

export const HandleSettlement = ((reactor: DassieReactor) => {
  const settlementSchemeManager = reactor.use(
    ManageSettlementSchemeInstancesActor,
  )
  const nodeTableStore = reactor.use(NodeTableStore)

  return async ({
    message: {
      sender,
      content: {
        value: {
          value: { settlementSchemeId, amount, settlementSchemeData },
        },
      },
    },
  }) => {
    const settlementSchemeActor =
      settlementSchemeManager.get(settlementSchemeId)

    if (!settlementSchemeActor) return

    const peerState = nodeTableStore.read().get(sender)?.peerState

    if (peerState?.id !== "peered") {
      throw new Error(`Settlement failed, peer ${sender} is not peered`)
    }

    await settlementSchemeActor.api.handleSettlement.ask({
      peerId: sender,
      amount,
      settlementSchemeData,
      peerState: peerState.settlementSchemeState,
    })
  }
}) satisfies PeerMessageHandler<"settlement">
