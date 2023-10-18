import { createActor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import { peerProtocol as logger } from "../../logger/instances"
import type { IncomingPeerMessageEvent } from "../actors/handle-peer-message"
import { NodeTableStore } from "../stores/node-table"
import { parseLinkStateEntries } from "../utils/parse-link-state-entries"
import { MAX_LINK_STATE_UPDATE_RETRANSMIT_DELAY } from "./link-state-update"

export const HandleRegistrationActor = () =>
  createActor((sig) => {
    const nodeTable = sig.use(NodeTableStore)

    return {
      handle: ({
        message: {
          content: {
            value: { value: content },
          },
        },
      }: IncomingPeerMessageEvent<"registration">) => {
        const { value: linkState, bytes: linkStateBytes } = content.nodeInfo
        const { nodeId, url, alias, sequence, entries, nodePublicKey } =
          linkState.signed

        if (nodeTable.read().has(nodeId)) {
          return EMPTY_UINT8ARRAY
        }

        const { neighbors, settlementSchemes } = parseLinkStateEntries(entries)

        logger.debug("received registration", {
          from: nodeId,
          sequence,
          neighbors: neighbors.join(","),
          settlementSchemes: settlementSchemes.join(","),
        })

        nodeTable.addNode({
          nodeId,
          url,
          alias,
          nodePublicKey,
          linkState: {
            sequence,
            updateReceivedCounter: 1,
            scheduledRetransmitTime:
              Date.now() +
              Math.ceil(Math.random() * MAX_LINK_STATE_UPDATE_RETRANSMIT_DELAY),
            neighbors,
            settlementSchemes,
            lastUpdate: linkStateBytes,
          },
          peerState: { id: "none" },
        })

        return EMPTY_UINT8ARRAY
      },
    }
  })
