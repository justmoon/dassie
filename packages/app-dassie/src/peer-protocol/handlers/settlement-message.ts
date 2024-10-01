import type { DassieReactor } from "../../base/types/dassie-base"
import { ManageSettlementSchemeInstancesActor } from "../../ledgers/manage-settlement-scheme-instances"
import type { PeerMessageHandler } from "../functions/handle-peer-message"

export const HandleSettlementMessage = ((reactor: DassieReactor) => {
  const settlementSchemeManager = reactor.use(
    ManageSettlementSchemeInstancesActor,
  )

  return ({
    message: {
      sender,
      content: {
        value: {
          value: { settlementSchemeId: settlementSchemeId, message },
        },
      },
    },
  }) => {
    const settlementSchemeActor =
      settlementSchemeManager.get(settlementSchemeId)

    if (!settlementSchemeActor) return

    settlementSchemeActor.api.handleMessage.tell({
      peerId: sender,
      message,
    })
  }
}) satisfies PeerMessageHandler<"settlementMessage">
