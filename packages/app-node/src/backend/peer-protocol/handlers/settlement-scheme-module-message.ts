import { createActor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import { ManageSettlementSchemeInstancesActor } from "../../settlement-schemes/manage-settlement-scheme-instances"
import type { IncomingPeerMessageEvent } from "../actors/handle-peer-message"

export const HandleSettlementSchemeModuleMessageActor = () =>
  createActor((sig) => {
    const settlementSchemeManager = sig.use(
      ManageSettlementSchemeInstancesActor,
    )

    return sig.handlers({
      handle: ({
        message: {
          sender,
          content: {
            value: {
              value: { settlementSchemeId: settlementSchemeId, message },
            },
          },
        },
      }: IncomingPeerMessageEvent<"settlementSchemeModuleMessage">) => {
        const settlementSchemeActor =
          settlementSchemeManager.get(settlementSchemeId)

        if (!settlementSchemeActor) return EMPTY_UINT8ARRAY

        settlementSchemeActor.api.handleMessage.tell({
          peerId: sender,
          message,
        })

        return EMPTY_UINT8ARRAY
      },
    })
  })
