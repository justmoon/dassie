import { Reactor } from "@dassie/lib-reactive"

import { peerProtocol as logger } from "../../logger/instances"
import type { PeerMessageHandler } from "../functions/handle-peer-message"
import { ModifyNodeTable } from "../functions/modify-node-table"

export const HandleRegistration = ((reactor: Reactor) => {
  return ({
    message: {
      content: {
        value: { value: content },
      },
    },
  }) => {
    const { value: linkState, bytes: linkStateBytes } = content.nodeInfo
    const { nodeId, sequence } = linkState.signed

    logger.debug("received registration", {
      from: nodeId,
      sequence,
    })

    const modifyNodeTable = reactor.use(ModifyNodeTable)
    modifyNodeTable.addNode(nodeId)
    modifyNodeTable.processLinkState({
      linkState: linkState.signed,
      linkStateBytes,
      retransmit: "never",
    })
  }
}) satisfies PeerMessageHandler<"registration">
