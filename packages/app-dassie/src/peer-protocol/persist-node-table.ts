import { castImmutable } from "immer"

import { createActor, watchStoreChanges } from "@dassie/lib-reactive"

import { Database } from "../database/open-database"
import { type NodeTableEntry, NodeTableStore } from "./stores/node-table"
import type { NodeId } from "./types/node-id"

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
      const settlementSchemeState = JSON.parse(
        row.settlement_scheme_state!,
      ) as unknown

      if (!settlementSchemeState || typeof settlementSchemeState !== "object") {
        throw new Error("Invalid settlement scheme state")
      }

      initialNodesMap.set(row.id, {
        nodeId: row.id,
        linkState: undefined,
        peerState:
          row.settlement_scheme_id ?
            {
              id: "peered",
              lastSeen: 0,
              settlementSchemeId: row.settlement_scheme_id,
              settlementSchemeState,
            }
          : { id: "none" },
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
