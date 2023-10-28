import { Reactor } from "@dassie/lib-reactive"

import { peerProtocol as logger } from "../../logger/instances"
import type { PeerMessageHandler } from "../actors/handle-peer-message"
import { ModifyNodeTableActor } from "../modify-node-table"

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

    const modifyNodeTableActor = reactor.use(ModifyNodeTableActor)
    modifyNodeTableActor.api.addNode.tell(nodeId)
    modifyNodeTableActor.api.processLinkState.tell({
      linkState: linkState.signed,
      linkStateBytes,
      retransmit: "never",
      from: nodeId,
    })
  }
}) satisfies PeerMessageHandler<"registration">
