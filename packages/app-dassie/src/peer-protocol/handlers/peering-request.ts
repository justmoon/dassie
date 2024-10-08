import type { DassieReactor } from "../../base/types/dassie-base"
import { EMPTY_UINT8ARRAY } from "../../constants/general"
import { ManageSettlementSchemeInstancesActor } from "../../ledgers/manage-settlement-scheme-instances"
import { ActiveSettlementSchemesSignal } from "../../ledgers/signals/active-settlement-schemes"
import type { PeerMessageHandler } from "../functions/handle-peer-message"
import { ProcessLinkState } from "../functions/modify-node-table"
import { NodeTableStore } from "../stores/node-table"

export const HandlePeeringRequest = ((reactor: DassieReactor) => {
  const nodeTableStore = reactor.use(NodeTableStore)
  const activeSettlementSchemesSignal = reactor.use(
    ActiveSettlementSchemesSignal,
  )
  const processLinkState = reactor.use(ProcessLinkState)

  return async ({
    message: {
      content: {
        value: { value: content },
      },
    },
  }) => {
    const { nodeInfo, settlementSchemeId, settlementSchemeData } = content

    const { nodeId } = nodeInfo.signed.value

    if (!activeSettlementSchemesSignal.read().has(settlementSchemeId)) {
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

    nodeTableStore.act.addNode(nodeId)
    processLinkState({
      linkState: nodeInfo.signed.value,
      linkStateBytes: nodeInfo.signed.bytes,
      retransmit: "never",
    })

    nodeTableStore.act.updateNode(nodeId, {
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
