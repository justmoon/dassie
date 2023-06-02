import { createActor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import { activeSubnetsSignal } from "../../subnets/signals/active-subnets"
import type { IncomingPeerMessageEvent } from "../actors/handle-peer-message"
import { nodeTableStore } from "../stores/node-table"

export const handlePeeringRequest = () =>
  createActor((sig) => {
    const nodeTable = sig.use(nodeTableStore)
    const activeSubnets = sig.get(activeSubnetsSignal)

    return {
      handle: ({
        message: {
          content: {
            value: { value: content },
          },
        },
      }: IncomingPeerMessageEvent<"peeringRequest">) => {
        const { nodeInfo, subnetId } = content

        const { nodeId, url, alias, nodePublicKey } = nodeInfo.signed

        if (!activeSubnets.has(subnetId)) return EMPTY_UINT8ARRAY

        const existingEntry = nodeTable.read().get(nodeId)

        if (existingEntry) {
          nodeTable.updateNode(nodeId, {
            peerState: { id: "peered", lastSeen: Date.now(), subnetId },
          })
        } else {
          nodeTable.addNode({
            nodeId,
            peerState: { id: "peered", lastSeen: Date.now(), subnetId },
            url,
            alias,
            nodePublicKey,
            linkState: {
              lastUpdate: undefined,
              neighbors: [],
              subnets: [],
              scheduledRetransmitTime: 0,
              sequence: 0n,
              updateReceivedCounter: 0,
            },
          })
        }

        return EMPTY_UINT8ARRAY
      },
    }
  })
