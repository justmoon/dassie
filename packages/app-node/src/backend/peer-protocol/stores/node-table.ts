import { castDraft, castImmutable, enableMapSet, produce } from "immer"

import { createStore } from "@dassie/lib-reactive"

import { NodeId } from "../types/node-id"
import { SubnetId } from "../types/subnet-id"

enableMapSet()

export interface NodeTableEntry {
  /**
   * ID of the node - unique in our subnet.
   */
  readonly nodeId: NodeId

  /**
   * Node's public key.
   */
  readonly nodePublicKey: Uint8Array

  /**
   * Node's publicly reachable URL.
   */
  readonly url: string

  /**
   * Node's human-readable alias.
   *
   * @remarks
   *
   * This alias is not verified and intended for debuggability only.
   */
  readonly alias: string

  /**
   * Latest known state of the node's links.
   */
  readonly linkState: LinkState

  /**
   * Current peering state between us and this node.
   */
  readonly peerState: PeerState
}

export type PeerState =
  | { readonly id: "none" }
  | {
      readonly id: "request-peering"
      readonly lastSeen: number
      readonly subnetId: SubnetId
    }
  | {
      readonly id: "peered"
      readonly lastSeen: number
      readonly subnetId: SubnetId
    }

export interface LinkState {
  /**
   * Binary copy of the most recent link state update.
   */
  readonly lastUpdate: Uint8Array | undefined

  /**
   * List of the node's peers.
   */
  readonly neighbors: readonly NodeId[]

  /**
   * List of the node's supported subnets.
   */
  readonly subnets: readonly SubnetId[]

  /**
   * Sequence number of the most recent link state update.
   */
  readonly sequence: bigint

  /**
   * How many times has the most recent link state update been received?
   */
  readonly updateReceivedCounter: number

  /**
   * Time when we will retransmit the most recent link state update unless the update received counter exceeds the threshold.
   */
  readonly scheduledRetransmitTime: number
}

export const nodeTableStore = () =>
  createStore(castImmutable(new Map<NodeId, NodeTableEntry>()), {
    addNode: (entry: NodeTableEntry) =>
      produce((draft) => {
        draft.set(
          entry.nodeId,
          castDraft({
            ...entry,
          })
        )
      }),
    updateNode: (key: NodeId, nodeEntry: Partial<NodeTableEntry>) =>
      produce((draft) => {
        const previousEntry = draft.get(key)
        if (previousEntry == undefined) {
          throw new Error("Node not found")
        }
        draft.set(
          key,
          castDraft({
            ...previousEntry,
            ...nodeEntry,
          })
        )
      }),
  })
