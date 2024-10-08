import type { Reactor } from "@dassie/lib-reactive"
import { isFailure } from "@dassie/lib-type-utils"

import { peerProtocol as logger } from "../../logger/instances"
import type { PeerMessageHandler } from "../functions/handle-peer-message"
import { ProcessLinkState } from "../functions/modify-node-table"
import { verifyLinkState } from "../functions/verify-link-state"

export const HandleLinkStateUpdate = ((reactor: Reactor) => {
  const processLinkState = reactor.use(ProcessLinkState)

  return async ({
    message: {
      sender,
      content: {
        value: { value: content },
      },
    },
  }) => {
    const { value: linkState, bytes: linkStateBytes } = content

    const verifyResult = await verifyLinkState({
      linkStateSignedBytes: linkState.signed.bytes,
      linkState: linkState.signed.value,
      signature: linkState.signature,
    })

    if (isFailure(verifyResult)) {
      // Ignore invalid link state
      logger.warn("received invalid link state update", {
        from: sender,
        error: verifyResult,
      })
      return
    }

    processLinkState({
      linkState: linkState.signed.value,
      linkStateBytes,
      retransmit: "scheduled",
    })

    return
  }
}) satisfies PeerMessageHandler<"linkStateUpdate">
