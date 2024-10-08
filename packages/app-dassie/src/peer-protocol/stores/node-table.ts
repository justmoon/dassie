import { castDraft, castImmutable, enableMapSet, produce } from "immer"

import { createStore } from "@dassie/lib-reactive"

import type { NodeId } from "../types/node-id"
import type { SettlementSchemeId } from "../types/settlement-scheme-id"

enableMapSet()

export interface NodeTableEntry {
  /**
   * ID of the node - globally unique within the Dassie realm.
   */
  readonly nodeId: NodeId

  /**
   * Latest known state of the node's links.
   */
  readonly linkState: LinkState | undefined

  /**
   * Current peering state between us and this node.
   */
  readonly peerState: PeerState

  /**
   * How the node was discovered. This affects whether it will be included in
   * the node list.
   */
  readonly registrationState: RegistrationState
}

export type PeerState =
  | { readonly id: "none" }
  | {
      readonly id: "peered"
      readonly lastSeen: number
      readonly settlementSchemeId: SettlementSchemeId
      readonly settlementSchemeState: object
    }

export interface LinkState {
  /**
   * Binary copy of the most recent link state update.
   */
  readonly lastUpdate: Uint8Array

  /**
   * Sequence number of the most recent link state update.
   */
  readonly sequence: bigint

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
   * Node's public key.
   */
  readonly publicKey: Uint8Array

  /**
   * List of the node's peers.
   */
  readonly neighbors: readonly NodeId[]

  /**
   * List of the node's supported settlement schemes.
   */
  readonly settlementSchemes: readonly SettlementSchemeId[]

  /**
   * How many times has the most recent link state update been received?
   */
  readonly updateReceivedCounter: number

  /**
   * Time when we will retransmit the most recent link state update unless the update received counter exceeds the threshold.
   */
  readonly scheduledRetransmitTime: number
}

export type DiscoverySource =
  | "self" // This node is us
  | "registration" // This node paid us to register itself
  | "majority" // This node is known by a majority of other nodes
  | "client" // This node is connected to us as a client

export type RegistrationState =
  | {
      id: "none"
    }
  | {
      id: "registered"
      registeredAt: Date
      renewedAt: Date
    }

export const NodeTableStore = () =>
  createStore(castImmutable(new Map<NodeId, NodeTableEntry>())).actions({
    /**
     * Notify the node table that a new node has been discovered.
     *
     * If the node is already known, this will do nothing.
     */
    addNode: (nodeId: NodeId) =>
      produce((draft) => {
        if (draft.has(nodeId)) return

        draft.set(
          nodeId,
          castDraft({
            nodeId,
            linkState: undefined,
            peerState: { id: "none" },
            registrationState: { id: "none" },
          }),
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
          }),
        )
      }),
    registerNode: (nodeId: NodeId) =>
      produce((draft) => {
        const entry = draft.get(nodeId)

        if (!entry) {
          throw new Error("Node not found")
        }

        draft.set(
          nodeId,
          castDraft({
            ...entry,
            registrationState: {
              id: "registered",
              registeredAt:
                entry.registrationState.id === "registered" ?
                  entry.registrationState.registeredAt
                : new Date(),
              renewedAt: new Date(),
            },
          }),
        )
      }),
  })
