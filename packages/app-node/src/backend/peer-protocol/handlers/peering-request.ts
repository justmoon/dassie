import { createActor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import { activeSettlementSchemesSignal } from "../../settlement-schemes/signals/active-settlement-schemes"
import type { IncomingPeerMessageEvent } from "../actors/handle-peer-message"
import { nodeTableStore } from "../stores/node-table"

export const handlePeeringRequest = () =>
  createActor((sig) => {
    const nodeTable = sig.use(nodeTableStore)
    const activeSubnets = sig.get(activeSettlementSchemesSignal)

    return {
      handle: ({
        message: {
          content: {
            value: { value: content },
          },
        },
      }: IncomingPeerMessageEvent<"peeringRequest">) => {
        const { nodeInfo, settlementSchemeId } = content

        const { nodeId, url, alias, nodePublicKey } = nodeInfo.signed

        if (!activeSubnets.has(settlementSchemeId)) return EMPTY_UINT8ARRAY

        const existingEntry = nodeTable.read().get(nodeId)

        if (existingEntry) {
          nodeTable.updateNode(nodeId, {
            peerState: {
              id: "peered",
              lastSeen: Date.now(),
              settlementSchemeId,
            },
          })
        } else {
          nodeTable.addNode({
            nodeId,
            peerState: {
              id: "peered",
              lastSeen: Date.now(),
              settlementSchemeId,
            },
            url,
            alias,
            nodePublicKey,
            linkState: {
              lastUpdate: undefined,
              neighbors: [],
              settlementSchemes: [],
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
