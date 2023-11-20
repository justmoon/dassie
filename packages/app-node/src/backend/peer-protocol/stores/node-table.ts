import { castDraft, castImmutable, enableMapSet, produce } from "immer"

import { Reactor, createStore } from "@dassie/lib-reactive"

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
   * Latest known state of the node's links.
   */
  readonly linkState: LinkState | undefined

  /**
   * Current peering state between us and this node.
   */
  readonly peerState: PeerState
}

export type PeerState =
  | { readonly id: "none" }
  | {
      readonly id: "peered"
      readonly lastSeen: number
      readonly settlementSchemeId: SettlementSchemeId
      readonly settlementSchemeState: unknown
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

export const NodeTableStore = (reactor: Reactor) => {
  const database = reactor.use(Database)

  const { rows: result } = database.executeSync(
    database.kysely
      .selectFrom("nodes")
      .select("id")
      .leftJoin("peers", "nodes.rowid", "peers.node")
      .select(["settlement_scheme_id", "settlement_scheme_state"])
      .compile(),
  )

  const initialNodesMap = new Map<NodeId, NodeTableEntry>()

  for (const row of result) {
    initialNodesMap.set(row.id, {
      nodeId: row.id,
      linkState: undefined,
      peerState: row.settlement_scheme_id
        ? {
            id: "peered",
            lastSeen: 0,
            settlementSchemeId: row.settlement_scheme_id,
            settlementSchemeState: JSON.parse(
              row.settlement_scheme_state!,
            ) as unknown,
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
