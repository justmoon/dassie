import type { TransactionStream } from "xrpl"

import type { Reactor } from "@dassie/lib-reactive"

import { NodeIdSignal } from "../../../../ilp-connector/computed/node-id"
import { settlementXrpl as logger } from "../../../../logger/instances"
import { NodeTableStore } from "../../../../peer-protocol/stores/node-table"
import type { NodeId } from "../../../../peer-protocol/types/node-id"
import { XRP_SETTLEMENT_MEMO_TYPE } from "../constants/settlement-memo-type"

type IsSettlementResult =
  | {
      isSettlement: false
    }
  | {
      isSettlement: true
      direction: "outgoing"
    }
  | {
      isSettlement: true
      direction: "incoming"
      peerId: NodeId
    }

export const IsSettlement = (reactor: Reactor) => {
  const nodeTableStore = reactor.use(NodeTableStore)
  const nodeIdSignal = reactor.use(NodeIdSignal)

  return function isSettlement(
    transaction: TransactionStream,
  ): IsSettlementResult {
    if (transaction.transaction.TransactionType !== "Payment") {
      return { isSettlement: false }
    }

    const settlementMemo = transaction.transaction.Memos?.find(
      (memo) =>
        memo.Memo.MemoType?.toUpperCase() ===
        XRP_SETTLEMENT_MEMO_TYPE.toUpperCase(),
    )

    if (!settlementMemo) {
      return { isSettlement: false }
    }

    const peerId = Buffer.from(
      settlementMemo.Memo.MemoData ?? "",
      "hex",
    ).toString() as NodeId

    if (peerId === nodeIdSignal.read()) {
      return { isSettlement: true, direction: "outgoing" }
    }

    const peerInfo = nodeTableStore.read().get(peerId)

    if (!peerInfo) {
      logger.warn("received settlement from unknown peer", {
        peerId,
      })
      return { isSettlement: false }
    }

    if (peerInfo.peerState.id !== "peered") {
      logger.warn("received settlement from unpeered node", {
        peerId,
      })
      return { isSettlement: false }
    }

    if (peerInfo.peerState.settlementSchemeId !== "xrpl-testnet") {
      logger.warn(
        "received settlement from node with wrong settlement scheme",
        {
          peerId,
          settlementSchemeId: peerInfo.peerState.settlementSchemeId,
        },
      )
      return { isSettlement: false }
    }

    return { isSettlement: true, direction: "incoming", peerId }
  }
}
