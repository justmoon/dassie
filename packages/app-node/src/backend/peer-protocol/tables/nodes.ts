import { column, table } from "@dassie/lib-sqlite"

import type { NodeId } from "../types/node-id"

export const nodesTable = table({
  name: "nodes",
  columns: {
    id: column().type("TEXT").typescriptType<NodeId>().primaryKey(),
    public_key: column().type("BLOB").notNull(),
    url: column().notNull().type("TEXT"),
    alias: column().notNull().type("TEXT"),
  },
})
