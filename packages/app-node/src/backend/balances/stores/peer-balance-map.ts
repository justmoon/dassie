import produce, { enableMapSet } from "immer"

import { createStore } from "@dassie/lib-reactive"

import type { NodeTableKey } from "../../peer-protocol/stores/node-table"

enableMapSet()

// TODO: Implement actual credit amounts
const DEFAULT_CREDIT = 100_000n

export interface PeerBalanceState {
  balance: bigint

  incomingPending: bigint

  outgoingPending: bigint

  totalTraffic: bigint

  incomingCredit: bigint

  outgoingCredit: bigint

  outgoingPendingSettlement: bigint
}

export const peerBalanceMapStore = () =>
  createStore(new Map<NodeTableKey, PeerBalanceState>(), {
    initializePeer: (peerKey: NodeTableKey) =>
      produce((draft) => {
        if (draft.has(peerKey)) {
          throw new Error("Cannot initialize peer that already exists")
        }

        draft.set(peerKey, {
          balance: 0n,
          incomingPending: 0n,
          outgoingPending: 0n,
          totalTraffic: 0n,
          incomingCredit: DEFAULT_CREDIT,
          outgoingCredit: DEFAULT_CREDIT,
          outgoingPendingSettlement: 0n,
        })
      }),
    removePeer: (peerKey: NodeTableKey) =>
      produce((draft) => {
        draft.delete(peerKey)
      }),

    handleIncomingPacketPrepared: (peerKey: NodeTableKey, amount: bigint) =>
      produce((draft) => {
        if (amount <= 0n) {
          throw new TypeError("Amount must be positive (>0)")
        }

        const currentBalance = draft.get(peerKey)

        if (!currentBalance) {
          throw new Error(
            "Cannot handle incoming prepare for peer that does not exist"
          )
        }

        currentBalance.incomingPending += amount
      }),
    handleIncomingPacketFulfilled: (peerKey: NodeTableKey, amount: bigint) =>
      produce((draft) => {
        if (amount <= 0n) {
          throw new TypeError("Amount must be positive (>0)")
        }

        const currentBalance = draft.get(peerKey)

        if (!currentBalance) {
          throw new Error(
            "Cannot handle incoming fulfill for peer that does not exist"
          )
        }

        currentBalance.incomingPending -= amount
        currentBalance.balance += amount
        currentBalance.totalTraffic += amount
      }),
    handleIncomingPacketRejected: (peerKey: NodeTableKey, amount: bigint) =>
      produce((draft) => {
        if (amount <= 0n) {
          throw new TypeError("Amount must be positive (>0)")
        }

        const currentBalance = draft.get(peerKey)

        if (!currentBalance) {
          throw new Error(
            "Cannot handle incoming reject for peer that does not exist"
          )
        }

        currentBalance.incomingPending -= amount
      }),

    handleOutgoingPacketPrepared: (peerKey: NodeTableKey, amount: bigint) =>
      produce((draft) => {
        if (amount <= 0n) {
          throw new TypeError("Amount must be positive (>0)")
        }

        const currentBalance = draft.get(peerKey)

        if (!currentBalance) {
          throw new Error(
            "Cannot handle outgoing prepare for peer that does not exist"
          )
        }

        currentBalance.outgoingPending += amount
      }),
    handleOutgoingPacketFulfilled: (peerKey: NodeTableKey, amount: bigint) =>
      produce((draft) => {
        if (amount <= 0n) {
          throw new TypeError("Amount must be positive (>0)")
        }

        const currentBalance = draft.get(peerKey)

        if (!currentBalance) {
          throw new Error(
            "Cannot handle outgoing fulfill for peer that does not exist"
          )
        }

        currentBalance.outgoingPending -= amount
        currentBalance.balance -= amount
        currentBalance.totalTraffic += amount
      }),
    handleOutgoingPacketRejected: (peerKey: NodeTableKey, amount: bigint) =>
      produce((draft) => {
        if (amount <= 0n) {
          throw new TypeError("Amount must be positive (>0)")
        }

        const currentBalance = draft.get(peerKey)

        if (!currentBalance) {
          throw new Error(
            "Cannot handle outgoing reject for peer that does not exist"
          )
        }

        currentBalance.outgoingPending -= amount
      }),

    handleOutgoingSettlementPrepared: (peerKey: NodeTableKey, amount: bigint) =>
      produce((draft) => {
        if (amount <= 0n) {
          throw new TypeError("Amount must be positive (>0)")
        }

        const currentBalance = draft.get(peerKey)

        if (!currentBalance) {
          throw new Error(
            "Cannot handle outgoing settlement for peer that does not exist"
          )
        }

        currentBalance.balance += amount
        currentBalance.outgoingPendingSettlement += amount
      }),

    handleOutgoingSettlementFulfilled: (
      peerKey: NodeTableKey,
      amount: bigint
    ) =>
      produce((draft) => {
        if (amount <= 0n) {
          throw new TypeError("Amount must be positive (>0)")
        }

        const currentBalance = draft.get(peerKey)

        if (!currentBalance) {
          throw new Error(
            "Cannot handle outgoing settlement for peer that does not exist"
          )
        }

        currentBalance.outgoingPendingSettlement -= amount
      }),

    handleOutgoingSettlementRejected: (peerKey: NodeTableKey, amount: bigint) =>
      produce((draft) => {
        if (amount <= 0n) {
          throw new TypeError("Amount must be positive (>0)")
        }

        const currentBalance = draft.get(peerKey)

        if (!currentBalance) {
          throw new Error(
            "Cannot handle outgoing settlement for peer that does not exist"
          )
        }

        currentBalance.outgoingPendingSettlement -= amount
      }),
  })
