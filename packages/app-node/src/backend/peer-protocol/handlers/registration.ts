import { Reactor } from "@dassie/lib-reactive"
import { isFailure } from "@dassie/lib-type-utils"

import { peerProtocol as logger } from "../../logger/instances"
import type { PeerMessageHandler } from "../functions/handle-peer-message"
import { ModifyNodeTable } from "../functions/modify-node-table"
import { verifyLinkState } from "../functions/verify-link-state"

export const HandleRegistration = ((reactor: Reactor) => {
  return async ({
    authenticated,
    message: {
      sender,
      content: {
        value: { value: content },
      },
    },
  }) => {
    const { value: linkState, bytes: linkStateBytes } = content.nodeInfo
    const { nodeId, sequence } = linkState.signed.value

    logger.debug?.("received registration", {
      from: nodeId,
      sequence,
    })

    const verifyResult = await verifyLinkState({
      linkStateSignedBytes: linkState.signed.bytes,
      linkState: linkState.signed.value,
      signature: linkState.signature,
    })

    if (isFailure(verifyResult)) {
      // Ignore invalid link state
      logger.warn("received invalid link state in registration", {
        from: sender,
        authenticated,
        error: verifyResult,
      })
      return
    }

    const modifyNodeTable = reactor.use(ModifyNodeTable)
    modifyNodeTable.addNode(nodeId)
    modifyNodeTable.processLinkState({
      linkState: linkState.signed.value,
      linkStateBytes,
      retransmit: "never",
    })
  }
}) satisfies PeerMessageHandler<"registration">
