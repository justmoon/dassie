import { Reactor } from "@dassie/lib-reactive"

import type { PeerMessageHandler } from "../functions/handle-peer-message"
import { ModifyNodeTable } from "../functions/modify-node-table"

export const HandleLinkStateUpdate = ((reactor: Reactor) => {
  const modifyNodeTable = reactor.use(ModifyNodeTable)

  return ({
    message: {
      content: {
        value: { value: content },
      },
    },
  }) => {
    const { value: linkState, bytes: linkStateBytes } = content

    modifyNodeTable.processLinkState({
      linkState: linkState.signed,
      linkStateBytes,
      retransmit: "scheduled",
    })

    return
  }
}) satisfies PeerMessageHandler<"linkStateUpdate">
