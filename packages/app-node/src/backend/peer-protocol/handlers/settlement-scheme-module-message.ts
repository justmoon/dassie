import { Reactor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import { ManageSettlementSchemeInstancesActor } from "../../settlement-schemes/manage-settlement-scheme-instances"
import type { PeerMessageHandler } from "../actors/handle-peer-message"

export const HandleSettlementSchemeModuleMessage = ((reactor: Reactor) => {
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

    if (!settlementSchemeActor) return EMPTY_UINT8ARRAY

    settlementSchemeActor.api.handleMessage.tell({
      peerId: sender,
      message,
    })

    return EMPTY_UINT8ARRAY
  }
}) satisfies PeerMessageHandler<"settlementSchemeModuleMessage">
