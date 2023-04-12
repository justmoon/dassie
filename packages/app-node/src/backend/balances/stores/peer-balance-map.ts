import produce, { enableMapSet } from "immer"

import { createStore } from "@dassie/lib-reactive"

import type { NodeTableKey } from "../../peer-protocol/stores/node-table"

enableMapSet()

export interface PeerBalanceState {
  balance: bigint

  incomingPending: bigint

  outgoingPending: bigint

  totalTraffic: bigint
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
        })
      }),
    removePeer: (peerKey: NodeTableKey) =>
      produce((draft) => {
        draft.delete(peerKey)
      }),

    handleIncomingPrepared: (peerKey: NodeTableKey, amount: bigint) =>
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
    handleIncomingFulfilled: (peerKey: NodeTableKey, amount: bigint) =>
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
    handleIncomingRejected: (peerKey: NodeTableKey, amount: bigint) =>
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

    handleOutgoingPrepared: (peerKey: NodeTableKey, amount: bigint) =>
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
    handleOutgoingFulfilled: (peerKey: NodeTableKey, amount: bigint) =>
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
    handleOutgoingRejected: (peerKey: NodeTableKey, amount: bigint) =>
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
  })
