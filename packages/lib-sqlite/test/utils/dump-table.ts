import type { AnyDatabase } from "../../src"

export const dumpTable = (database: AnyDatabase, tableName: string) => {
  const rows = database.raw.prepare(`SELECT * FROM ${tableName}`).all()

  return rows
}
