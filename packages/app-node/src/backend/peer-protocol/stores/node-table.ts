import produce, { enableMapSet } from "immer"

import { createStore } from "@dassie/lib-reactive"

enableMapSet()

export interface NodeTableEntry {
  /**
   * ID of the node - unique in our subnet.
   */
  nodeId: string

  /**
   * Node's public key.
   */
  nodeKey: Uint8Array

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

export const nodeTableStore = () =>
  createStore(new Map<string, NodeTableEntry>(), {
    addNode: (entry: NodeTableEntry) =>
      produce((draft) => {
        draft.set(entry.nodeId, {
          ...entry,
        })
      }),
    updateNode: (nodeId: string, nodeEntry: Partial<NodeTableEntry>) =>
      produce((draft) => {
        const previousEntry = draft.get(nodeId)
        if (previousEntry == undefined) {
          throw new Error("nodeId not found")
        }
        draft.set(nodeId, {
          ...previousEntry,
          ...nodeEntry,
        })
      }),
  })
