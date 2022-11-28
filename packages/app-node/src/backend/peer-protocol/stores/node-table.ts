import produce, { enableMapSet } from "immer"

import { createStore } from "@dassie/lib-reactive"

enableMapSet()

export interface NodeTableEntry {
  /**
   * Subnet that the node is a part of.
   */
  subnetId: string

  /**
   * ID of the node - unique in our subnet.
   */
  nodeId: string

  /**
   * Node's public key.
   */
  nodePublicKey: Uint8Array

  /**
   * Sequence number of the most recent link state update.
   */
  sequence: bigint

  /**
   * How many times has the most recent link state update been received?
   */
  updateReceivedCounter: number

  /**
   * Time when we will retransmit the most recent link state update unless the update received counter exceeds the threshold.
   */
  scheduledRetransmitTime: number

  /**
   * List of the node's peers.
   */
  neighbors: string[]

  lastLinkStateUpdate: Uint8Array
}

export type NodeTableKey = `${string}.${string}`

export const nodeTableStore = () =>
  createStore(new Map<NodeTableKey, NodeTableEntry>(), {
    addNode: (entry: NodeTableEntry) =>
      produce((draft) => {
        draft.set(`${entry.subnetId}.${entry.nodeId}`, {
          ...entry,
        })
      }),
    updateNode: (key: NodeTableKey, nodeEntry: Partial<NodeTableEntry>) =>
      produce((draft) => {
        const previousEntry = draft.get(key)
        if (previousEntry == undefined) {
          throw new Error("Node not found")
        }
        draft.set(key, {
          ...previousEntry,
          ...nodeEntry,
        })
      }),
  })
