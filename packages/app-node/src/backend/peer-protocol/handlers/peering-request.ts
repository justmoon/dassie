import { createActor } from "@dassie/lib-reactive"

import { ActiveSettlementSchemesSignal } from "../../settlement-schemes/signals/active-settlement-schemes"
import type { IncomingPeerMessageEvent } from "../actors/handle-peer-message"
import { NodeTableStore } from "../stores/node-table"

export const HandlePeeringRequestActor = () =>
  createActor((sig) => {
    const nodeTable = sig.use(NodeTableStore)
    const activeSubnets = sig.get(ActiveSettlementSchemesSignal)

    return sig.handlers({
      handle: ({
        message: {
          content: {
            value: { value: content },
          },
        },
      }: IncomingPeerMessageEvent<"peeringRequest">) => {
        const { nodeInfo, settlementSchemeId } = content

        const { nodeId } = nodeInfo.signed

        if (!activeSubnets.has(settlementSchemeId)) {
          return new Uint8Array([0x00])
        }

        const existingEntry = nodeTable.read().get(nodeId)

        if (!existingEntry) {
          return new Uint8Array([0x00])
        }

        nodeTable.updateNode(nodeId, {
          peerState: {
            id: "peered",
            lastSeen: Date.now(),
            settlementSchemeId,
          },
        })

        return new Uint8Array([0x01])
      },
    })
  })
