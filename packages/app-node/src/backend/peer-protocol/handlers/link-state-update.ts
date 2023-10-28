import { Reactor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import type { PeerMessageHandler } from "../actors/handle-peer-message"
import { ModifyNodeTableActor } from "../modify-node-table"

export const HandleLinkStateUpdate = ((reactor: Reactor) => {
  const modifyNodeTableActor = reactor.use(ModifyNodeTableActor)

  return ({
    message: {
      sender,
      content: {
        value: { value: content },
      },
    },
  }) => {
    const { value: linkState, bytes: linkStateBytes } = content

    modifyNodeTableActor.api.processLinkState.tell({
      linkState: linkState.signed,
      linkStateBytes,
      retransmit: "scheduled",
      from: sender,
    })

    return EMPTY_UINT8ARRAY
  }
}) satisfies PeerMessageHandler<"linkStateUpdate">
