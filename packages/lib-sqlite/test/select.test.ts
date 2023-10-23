import { describe, test } from "vitest"

import minimal from "./schemas/minimal"
import { createTestDatabase } from "./utils/create-test-database"

describe("SELECT", () => {
  describe("with a minimal schema", () => {
    test("should select all rows using kysely", async ({ expect }) => {
      const database = createTestDatabase(minimal)

      const rows = await database.kysely
        .selectFrom("users")
        .selectAll()
        .execute()

      expect(rows).toMatchInlineSnapshot(`
        [
          {
            "age": 42n,
            "id": 1n,
            "name": "Anzhela",
          },
          {
            "age": 21n,
            "id": 2n,
            "name": "Borna",
          },
          {
            "age": 42n,
            "id": 3n,
            "name": "Caelius",
          },
        ]
      `)
    })

    test("should select all rows using shorthand", ({ expect }) => {
      const database = createTestDatabase(minimal)

      const rows = database.tables.users.select()

      expect(rows).toMatchInlineSnapshot(`
        [
          {
            "age": 42n,
            "id": 1n,
            "name": "Anzhela",
          },
          {
            "age": 21n,
            "id": 2n,
            "name": "Borna",
          },
          {
            "age": 42n,
            "id": 3n,
            "name": "Caelius",
          },
        ]
      `)
    })

    test("should select a subset of rows using kysely", async ({ expect }) => {
      const database = createTestDatabase(minimal)

      const rows = await database.kysely
        .selectFrom("users")
        .select(["name"])
        .where("age", "=", 42n)
        .execute()

      expect(rows).toMatchInlineSnapshot(`
        [
          {
            "name": "Anzhela",
          },
          {
            "name": "Caelius",
          },
        ]
      `)
    })

    test("should select a subset of rows using shorthand", ({ expect }) => {
      const database = createTestDatabase(minimal)

      const rows = database.tables.users.select({ age: 42n })

      expect(rows).toMatchInlineSnapshot(`
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

    test("should select a single row using kysely", async ({ expect }) => {
      const database = createTestDatabase(minimal)

      const row = await database.kysely
        .selectFrom("users")
        .select(["name"])
        .where("id", "=", 2n)
        .executeTakeFirst()

      expect(row).toMatchInlineSnapshot(`
        {
          "name": "Borna",
        }
      `)
    })

    test("should select a single row using shorthand", ({ expect }) => {
      const database = createTestDatabase(minimal)

      const row = database.tables.users.selectFirst({ id: 2n })

      expect(row).toMatchInlineSnapshot(`
        {
          "age": 21n,
          "id": 2n,
          "name": "Borna",
        }
      `)
    })
  })
})
