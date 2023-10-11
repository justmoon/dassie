import { castDraft, castImmutable, enableMapSet, produce } from "immer"

import { Reactor, createStore } from "@dassie/lib-reactive"
import { InferRowSqliteType } from "@dassie/lib-sqlite"

import { Database } from "../../database/open-database"
import { NodeId } from "../types/node-id"
import { SettlementSchemeId } from "../types/settlement-scheme-id"

enableMapSet()

export interface NodeTableEntry {
  /**
   * ID of the node - globally unique within the Dassie realm.
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
      readonly settlementSchemeId: SettlementSchemeId
    }
  | {
      readonly id: "peered"
      readonly lastSeen: number
      readonly settlementSchemeId: SettlementSchemeId
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
   * List of the node's supported settlement schemes.
   */
  readonly settlementSchemes: readonly SettlementSchemeId[]

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

export const NodeTableStore = (reactor: Reactor) => {
  const database = reactor.use(Database)

  const result = database.raw
    .prepare(
      `SELECT nodes.*, peers.* FROM nodes LEFT JOIN peers ON nodes.rowid = peers.node`,
    )
    .all() as (InferRowSqliteType<typeof database.schema.tables.nodes> &
    Partial<InferRowSqliteType<typeof database.schema.tables.peers>>)[]

  const initialNodesMap = new Map<NodeId, NodeTableEntry>()

  for (const row of result) {
    initialNodesMap.set(row.id as NodeId, {
      nodeId: row.id as NodeId,
      nodePublicKey: new Uint8Array(
        row.public_key.buffer,
        row.public_key.byteOffset,
        row.public_key.byteLength,
      ),
      url: row.url,
      alias: row.alias,
      linkState: {
        lastUpdate: undefined,
        neighbors: [],
        settlementSchemes: [],
        sequence: 0n,
        updateReceivedCounter: 0,
        scheduledRetransmitTime: 0,
      },
      peerState: row.node
        ? {
            id: "peered",
            lastSeen: 0,
            settlementSchemeId: row.settlement_scheme_id! as SettlementSchemeId,
          }
        : { id: "none" },
    })
  }

  return createStore(castImmutable(initialNodesMap), {
    addNode: (entry: NodeTableEntry) =>
      produce((draft) => {
        draft.set(
          entry.nodeId,
          castDraft({
            ...entry,
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
  })
}
