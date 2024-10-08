import type { Reactor } from "@dassie/lib-reactive"
import { isFailure } from "@dassie/lib-type-utils"

import { peerProtocol as logger } from "../../logger/instances"
import type { PeerMessageHandler } from "../functions/handle-peer-message"
import { ProcessLinkState } from "../functions/modify-node-table"
import { verifyLinkState } from "../functions/verify-link-state"
import { NodeTableStore } from "../stores/node-table"

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

    const nodeTableStore = reactor.use(NodeTableStore)
    const processLinkState = reactor.use(ProcessLinkState)
    nodeTableStore.act.addNode(nodeId)
    nodeTableStore.act.registerNode(nodeId)
    processLinkState({
      linkState: linkState.signed.value,
      linkStateBytes,
      retransmit: "never",
    })
  }
}) satisfies PeerMessageHandler<"registration">
