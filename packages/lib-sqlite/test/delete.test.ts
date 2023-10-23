import { describe, test } from "vitest"

import minimal from "./schemas/minimal"
import { createTestDatabase } from "./utils/create-test-database"
import { dumpTable } from "./utils/dump-table"

describe("DELETE", () => {
  describe("with a minimal schema", () => {
    test("should delete all rows using kysely", async ({ expect }) => {
      const database = createTestDatabase(minimal)

      await database.kysely.deleteFrom("users").execute()

      expect(dumpTable(database, "users")).toMatchInlineSnapshot("[]")
    })

    test("should delete all rows using shorthand", ({ expect }) => {
      const database = createTestDatabase(minimal)

      database.tables.users.deleteAll()

      expect(dumpTable(database, "users")).toMatchInlineSnapshot("[]")
    })

    test("should delete a subset of rows using kysely", async ({ expect }) => {
      const database = createTestDatabase(minimal)

      await database.kysely.deleteFrom("users").where("age", "=", 42n).execute()

      expect(dumpTable(database, "users")).toMatchInlineSnapshot(`
        [
          {
            "age": 21n,
            "id": 2n,
            "name": "Borna",
          },
        ]
      `)
    })

    test("should delete a subset of rows using shorthand", ({ expect }) => {
      const database = createTestDatabase(minimal)

      database.tables.users.delete({ age: 42n })

      expect(dumpTable(database, "users")).toMatchInlineSnapshot(`
        [
          {
            "age": 21n,
            "id": 2n,
            "name": "Borna",
          },
        ]
      `)
    })

    test("should delete a single row using kysely", async ({ expect }) => {
      const database = createTestDatabase(minimal)

      await database.kysely
        .deleteFrom("users")
        .where("id", "=", 2n)
        .executeTakeFirst()

      expect(dumpTable(database, "users")).toMatchInlineSnapshot(`
        [
          {
            "age": 42n,
            "id": 1n,
            "name": "Anzhela",
          },
          {
            "age": 42n,
            "id": 3n,
            "name": "Caelius",
          },
        ]
      `)
    })

    test("should delete a single row using shorthand", ({ expect }) => {
      const database = createTestDatabase(minimal)

      database.tables.users.delete({ id: 2n })

      expect(dumpTable(database, "users")).toMatchInlineSnapshot(`
        [
          {
            "age": 42n,
            "id": 1n,
            "name": "Anzhela",
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
