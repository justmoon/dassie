import { createActor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import { peerProtocol as logger } from "../../logger/instances"
import type { IncomingPeerMessageEvent } from "../actors/handle-peer-message"
import { ModifyNodeTableActor } from "../modify-node-table"

export const HandleRegistrationActor = () =>
  createActor((sig) => {
    return {
      handle: ({
        message: {
          content: {
            value: { value: content },
          },
        },
      }: IncomingPeerMessageEvent<"registration">) => {
        const { value: linkState, bytes: linkStateBytes } = content.nodeInfo
        const { nodeId, sequence } = linkState.signed

        logger.debug("received registration", {
          from: nodeId,
          sequence,
        })

        const modifyNodeTableActor = sig.use(ModifyNodeTableActor)
        modifyNodeTableActor.api.addNode.tell(nodeId)
        modifyNodeTableActor.api.processLinkState.tell({
          linkState: linkState.signed,
          linkStateBytes,
          retransmit: "never",
          from: nodeId,
        })

        return EMPTY_UINT8ARRAY
      },
    }
  })
