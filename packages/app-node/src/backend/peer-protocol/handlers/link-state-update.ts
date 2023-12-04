import { Reactor } from "@dassie/lib-reactive"

import type { PeerMessageHandler } from "../functions/handle-peer-message"
import { ModifyNodeTableActor } from "../modify-node-table"

export const HandleLinkStateUpdate = ((reactor: Reactor) => {
  const modifyNodeTableActor = reactor.use(ModifyNodeTableActor)

  return ({
    message: {
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
    })

    return
  }
}) satisfies PeerMessageHandler<"linkStateUpdate">
