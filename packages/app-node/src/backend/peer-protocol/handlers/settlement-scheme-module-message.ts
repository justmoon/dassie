import { createActor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import { manageSettlementSchemeInstances } from "../../settlement-schemes/manage-settlement-scheme-instances"
import type { IncomingPeerMessageEvent } from "../actors/handle-peer-message"

export const handleSettlementSchemeModuleMessage = () =>
  createActor((sig) => {
    const settlementSchemeManager = sig.use(manageSettlementSchemeInstances)

    return {
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

        settlementSchemeActor.tell("handleMessage", {
          peerId: sender,
          message,
        })

        return EMPTY_UINT8ARRAY
      },
    }
  })
