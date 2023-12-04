import { Reactor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import { ManageSettlementSchemeInstancesActor } from "../../settlement-schemes/manage-settlement-scheme-instances"
import { ActiveSettlementSchemesSignal } from "../../settlement-schemes/signals/active-settlement-schemes"
import type { PeerMessageHandler } from "../functions/handle-peer-message"
import { NodeTableStore } from "../stores/node-table"

export const HandlePeeringRequest = ((reactor: Reactor) => {
  const nodeTableStore = reactor.use(NodeTableStore)
  const activeSettlementSchemesSignal = reactor.use(
    ActiveSettlementSchemesSignal,
  )

  return async ({
    message: {
      content: {
        value: { value: content },
      },
    },
  }) => {
    const { nodeInfo, settlementSchemeId, settlementSchemeData } = content

    const { nodeId } = nodeInfo.signed

    if (!activeSettlementSchemesSignal.read().has(settlementSchemeId)) {
      return { accepted: false, data: EMPTY_UINT8ARRAY }
    }

    const existingEntry = nodeTableStore.read().get(nodeId)

    if (!existingEntry) {
      return { accepted: false, data: EMPTY_UINT8ARRAY }
    }

    const acceptPeeringRequestResult = await reactor
      .use(ManageSettlementSchemeInstancesActor)
      .get(settlementSchemeId)
      ?.api.acceptPeeringRequest.ask({
        peerId: nodeId,
        data: settlementSchemeData,
      })

    if (!acceptPeeringRequestResult) {
      return { accepted: false, data: EMPTY_UINT8ARRAY }
    }

    nodeTableStore.updateNode(nodeId, {
      peerState: {
        id: "peered",
        lastSeen: Date.now(),
        settlementSchemeId,
        settlementSchemeState: acceptPeeringRequestResult.peerState,
      },
    })

    return {
      accepted: true,
      data: acceptPeeringRequestResult.peeringResponseData,
    }
  }
}) satisfies PeerMessageHandler<"peeringRequest">
