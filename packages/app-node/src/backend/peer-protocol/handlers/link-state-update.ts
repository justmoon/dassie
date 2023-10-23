import { createActor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import type { IncomingPeerMessageEvent } from "../actors/handle-peer-message"
import { ModifyNodeTableActor } from "../modify-node-table"

export const HandleLinkStateUpdateActor = () =>
  createActor((sig) => {
    const modifyNodeTableActor = sig.use(ModifyNodeTableActor)

    return {
      handle: ({
        message: {
          sender,
          content: {
            value: { value: content },
          },
        },
      }: IncomingPeerMessageEvent<"linkStateUpdate">) => {
        const { value: linkState, bytes: linkStateBytes } = content

        modifyNodeTableActor.tell("processLinkState", {
          linkState: linkState.signed,
          linkStateBytes,
          retransmit: "scheduled",
          from: sender,
        })

        return EMPTY_UINT8ARRAY
      },
    }
  })
