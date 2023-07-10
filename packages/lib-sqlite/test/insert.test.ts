import { describe, test } from "vitest"

import minimal from "./schemas/minimal"
import { createTestDatabase } from "./utils/create-test-database"
import { dumpTable } from "./utils/dump-table"

describe("INSERT query builder", () => {
  describe("with a minimal schema", () => {
    test("should insert a row", ({ expect }) => {
      const database = createTestDatabase(minimal)

      database.tables.users.insertOne({
        id: 1n,
        name: "Alice",
      })

      expect(dumpTable(database, "users")).toMatchInlineSnapshot(`
        [
          {
            "id": 1n,
            "name": "Alice",
          },
        ]
      `)
    })
  })
})
