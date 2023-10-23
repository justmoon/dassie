import { describe, test } from "vitest"

import minimal from "./schemas/minimal"
import { createTestDatabase } from "./utils/create-test-database"
import { dumpTable } from "./utils/dump-table"

describe("INSERT ON CONFLICT DO UPDATE", () => {
  describe("with a minimal schema", () => {
    test("should upsert a row using shorthand", ({ expect }) => {
      const database = createTestDatabase(minimal)

      database.tables.users.upsert(
        {
          id: 2n,
          name: "Bruno",
          age: 22n,
        },
        ["id"],
      )

      expect(dumpTable(database, "users")).toMatchInlineSnapshot(`
        [
          {
            "age": 42n,
            "id": 1n,
            "name": "Anzhela",
          },
          {
            "age": 22n,
            "id": 2n,
            "name": "Bruno",
          },
          {
            "age": 42n,
            "id": 3n,
            "name": "Caelius",
          },
        ]
      `)
    })
  })
})
