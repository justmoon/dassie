import { createActor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import type { IncomingPeerMessageEvent } from "../actions/handle-peer-message"
import { NodeTableKey, nodeTableStore } from "../stores/node-table"

export const handlePeeringRequest = () =>
  createActor((sig) => {
    const nodeTable = sig.use(nodeTableStore)
    return ({
      message: {
        content: {
          value: { value: content },
        },
      },
    }: IncomingPeerMessageEvent<"peeringRequest">) => {
      const { nodeInfo } = content

      const { subnetId, nodeId, url, nodePublicKey } = nodeInfo.signed

      const nodeKey: NodeTableKey = `${subnetId}.${nodeId}`
      const existingEntry = nodeTable.read().get(nodeKey)

      if (existingEntry) {
        nodeTable.updateNode(nodeKey, {
          peerState: { id: "peered", lastSeen: Date.now() },
        })
      } else {
        nodeTable.addNode({
          subnetId,
          nodeId,
          peerState: { id: "peered", lastSeen: Date.now() },
          url,
          nodePublicKey,
          linkState: {
            lastUpdate: undefined,
            neighbors: [],
            scheduledRetransmitTime: 0,
            sequence: 0n,
            updateReceivedCounter: 0,
          },
        })
      }

      return EMPTY_UINT8ARRAY
    }
  })
