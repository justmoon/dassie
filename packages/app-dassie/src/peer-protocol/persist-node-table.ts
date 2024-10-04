import { castImmutable } from "immer"

import { createActor, watchStoreChanges } from "@dassie/lib-reactive"

import { Database } from "../database/open-database"
import { type NodeTableEntry, NodeTableStore } from "./stores/node-table"
import type { NodeId } from "./types/node-id"
import type { SettlementSchemeId } from "./types/settlement-scheme-id"

function formatPeerState(
  settlementSchemeId: SettlementSchemeId | null,
  settlementSchemeState: string | null,
) {
  if (!settlementSchemeId || !settlementSchemeState) {
    return { id: "none" as const }
  }

  const settlementSchemeStateParsed = JSON.parse(
    settlementSchemeState,
  ) as unknown

  if (
    !settlementSchemeStateParsed ||
    typeof settlementSchemeStateParsed !== "object"
  ) {
    throw new Error("Invalid settlement scheme state")
  }

  return {
    id: "peered" as const,
    lastSeen: 0,
    settlementSchemeId,
    settlementSchemeState: settlementSchemeStateParsed,
  }
}

export const PersistNodeTableActor = () =>
  createActor((sig) => {
    const database = sig.reactor.use(Database)
    const nodeTableStore = sig.reactor.use(NodeTableStore)

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
        peerState: formatPeerState(
          row.settlement_scheme_id,
          row.settlement_scheme_state,
        ),
      })
    }

    nodeTableStore.write(castImmutable(initialNodesMap))

    watchStoreChanges(sig, nodeTableStore, {
      addNode: () => {
        // TODO
      },
      updateNode: () => {
        // TODO
      },
    })
  })
