import { enableMapSet, produce } from "immer"

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
   * Node's publicly reachable URL.
   */
  url: string

  /**
   * Node's human-readable alias.
   *
   * @remarks
   *
   * This alias is not verified and intended for debuggability only.
   */
  alias: string

  /**
   * Latest known state of the node's links.
   */
  linkState: LinkState

  /**
   * Current peering state between us and this node.
   */
  peerState: PeerState
}

export type PeerState =
  | { id: "none" }
  | {
      id: "request-peering"
      lastSeen: number
    }
  | {
      id: "peered"
      lastSeen: number
    }

export interface LinkState {
  /**
   * Binary copy of the most recent link state update.
   */
  lastUpdate: Uint8Array | undefined

  /**
   * List of the node's peers.
   */
  neighbors: string[]

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

export const parseNodeKey = (
  key: NodeTableKey
): [subnetId: string, nodeId: string] => {
  const subnetId = key.slice(0, key.indexOf("."))
  const nodeId = key.slice(key.indexOf(".") + 1)
  return [subnetId, nodeId]
}
