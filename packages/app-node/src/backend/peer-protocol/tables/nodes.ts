import { column, table } from "@dassie/lib-sqlite"

import { NodeId } from "../types/node-id"

export const nodesTable = table({
  name: "nodes",
  columns: {
    id: column()
      .type("TEXT")
      .primaryKey()
      .serialize((value: NodeId) => value)
      .deserialize((value) => value as NodeId),
    public_key: column()
      .type("BLOB")
      .notNull()
      .serialize((value: Uint8Array) => Buffer.from(value))
      .deserialize(
        (value) =>
          new Uint8Array(value.buffer, value.byteOffset, value.byteLength)
      ),
    url: column().notNull().type("TEXT"),
    alias: column().notNull().type("TEXT"),
  },
})
