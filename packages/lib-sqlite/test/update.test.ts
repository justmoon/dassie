import { describe, test } from "vitest"

import minimal from "./schemas/minimal"
import { createTestDatabase } from "./utils/create-test-database"
import { dumpTable } from "./utils/dump-table"

describe("SELECT", () => {
  describe("with a minimal schema", () => {
    test("should update all rows using kysely", async ({ expect }) => {
      const database = createTestDatabase(minimal)

      await database.kysely
        .updateTable("users")
        .set({
          age: 100n,
        })
        .execute()

      expect(dumpTable(database, "users")).toMatchInlineSnapshot(`
        [
          {
            "age": 100n,
            "id": 1n,
            "name": "Anzhela",
          },
          {
            "age": 100n,
            "id": 2n,
            "name": "Borna",
          },
          {
            "age": 100n,
            "id": 3n,
            "name": "Caelius",
          },
        ]
      `)
    })

    test("should update all rows using shorthand", ({ expect }) => {
      const database = createTestDatabase(minimal)

      database.tables.users.updateAll({
        age: 100n,
      })

      expect(dumpTable(database, "users")).toMatchInlineSnapshot(`
        [
          {
            "age": 100n,
            "id": 1n,
            "name": "Anzhela",
          },
          {
            "age": 100n,
            "id": 2n,
            "name": "Borna",
          },
          {
            "age": 100n,
            "id": 3n,
            "name": "Caelius",
          },
        ]
      `)
    })

    test("should update a subset of rows using kysely", async ({ expect }) => {
      const database = createTestDatabase(minimal)

      await database.kysely
        .updateTable("users")
        .set({ age: 100n })
        .where("age", "=", 42n)
        .execute()

      expect(dumpTable(database, "users")).toMatchInlineSnapshot(`
        [
          {
            "age": 100n,
            "id": 1n,
            "name": "Anzhela",
          },
          {
            "age": 21n,
            "id": 2n,
            "name": "Borna",
          },
          {
            "age": 100n,
            "id": 3n,
            "name": "Caelius",
          },
        ]
      `)
    })

    test("should update a subset of rows using shorthand", ({ expect }) => {
      const database = createTestDatabase(minimal)

      database.tables.users.update({ age: 42n }, { age: 100n })

      expect(dumpTable(database, "users")).toMatchInlineSnapshot(`
        [
          {
            "age": 100n,
            "id": 1n,
            "name": "Anzhela",
          },
          {
            "age": 21n,
            "id": 2n,
            "name": "Borna",
          },
          {
            "age": 100n,
            "id": 3n,
            "name": "Caelius",
          },
        ]
      `)
    })
  })
})
