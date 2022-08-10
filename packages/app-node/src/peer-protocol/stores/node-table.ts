import { createStore } from "@dassie/lib-reactive"
import produce, { enableMapSet } from "immer"

enableMapSet()

export interface NodeTableEntry {
  /**
   * ID of the node - unique in our subnet.
   */
  nodeId: string

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

export type Model = Map<string, NodeTableEntry>

export const nodeTableStore = () => createStore<Model>(new Map())

export const addNode = (nodeEntry: NodeTableEntry) =>
  produce<Model>((draft) => {
    draft.set(nodeEntry.nodeId, {
      ...nodeEntry,
    })
  })

export const updateNode = (
  nodeId: string,
  nodeEntry: Partial<NodeTableEntry>
) =>
  produce<Model>((draft) => {
    const previousEntry = draft.get(nodeId)
    if (previousEntry == undefined) {
      throw new Error("nodeId not found")
    }
    draft.set(nodeId, {
      ...previousEntry,
      ...nodeEntry,
    })
  })
