import { Reactor } from "@dassie/lib-reactive"

import { ActiveSettlementSchemesSignal } from "../../settlement-schemes/signals/active-settlement-schemes"
import type { PeerMessageHandler } from "../actors/handle-peer-message"
import { NodeTableStore } from "../stores/node-table"

export const HandlePeeringRequest = ((reactor: Reactor) => {
  const nodeTableStore = reactor.use(NodeTableStore)
  const activeSettlementSchemesSignal = reactor.use(
    ActiveSettlementSchemesSignal,
  )

  return ({
    message: {
      content: {
        value: { value: content },
      },
    },
  }) => {
    const { nodeInfo, settlementSchemeId } = content

    const { nodeId } = nodeInfo.signed

    if (!activeSettlementSchemesSignal.read().has(settlementSchemeId)) {
      return { accepted: false }
    }

    const existingEntry = nodeTableStore.read().get(nodeId)

    if (!existingEntry) {
      return { accepted: false }
    }

    nodeTableStore.updateNode(nodeId, {
      peerState: {
        id: "peered",
        lastSeen: Date.now(),
        settlementSchemeId,
      },
    })

    return { accepted: true }
  }
}) satisfies PeerMessageHandler<"peeringRequest">
