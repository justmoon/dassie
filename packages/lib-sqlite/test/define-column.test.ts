import type { Tagged } from "type-fest"
import { assertType, describe, test } from "vitest"

import { type ColumnDescriptionBuilder, column } from "../src"

describe("column schema builder", () => {
  test("should start with a default column definition", ({ expect }) => {
    const example = column()

    expect(example.description).toMatchInlineSnapshot(`
      {
        "defaultValue": undefined,
        "hasDefault": false,
        "notNull": false,
        "primaryKey": false,
        "type": "TEXT",
      }
    `)
  })

  test("should allow setting the type", ({ expect }) => {
    const example = column().type("INTEGER")

    expect(example.description).toMatchInlineSnapshot(`
      {
        "defaultValue": undefined,
        "hasDefault": false,
        "notNull": false,
        "primaryKey": false,
        "type": "INTEGER",
      }
    `)
  })

  test("should allow setting the required flag", ({ expect }) => {
    const example = column().notNull()

    expect(example.description).toMatchInlineSnapshot(`
      {
        "defaultValue": undefined,
        "hasDefault": false,
        "notNull": true,
        "primaryKey": false,
        "type": "TEXT",
      }
    `)
  })

  test("should allow setting the primary key flag", ({ expect }) => {
    const example = column().primaryKey()

    expect(example.description).toMatchInlineSnapshot(`
      {
        "defaultValue": undefined,
        "hasDefault": false,
        "notNull": false,
        "primaryKey": true,
        "type": "TEXT",
      }
    `)
  })

  test("should allow setting the typescript type", ({ expect }) => {
    const example = column().typescriptType<string>()

    expect(example.description).toMatchInlineSnapshot(`
      {
        "defaultValue": undefined,
        "hasDefault": false,
        "notNull": false,
        "primaryKey": false,
        "type": "TEXT",
      }
    `)
  })

  test("should allow setting the type, required flag and primary key flag", ({
    expect,
  }) => {
    const example = column().type("INTEGER").notNull().primaryKey()

    expect(example.description).toMatchInlineSnapshot(`
      {
        "defaultValue": undefined,
        "hasDefault": false,
        "notNull": true,
        "primaryKey": true,
        "type": "INTEGER",
      }
    `)
  })

  test("should allow setting the type, required flag, primary key flag, serialize function, and deserialize function", ({
    expect,
  }) => {
    type BrandedInteger = Tagged<bigint, "BrandedInteger">

    const example = column()
      .type("INTEGER")
      .typescriptType<BrandedInteger>()
      .notNull()
      .primaryKey()

    assertType<
      ColumnDescriptionBuilder<{
        sqliteType: "INTEGER"
        typescriptType: BrandedInteger
        notNull: true
        primaryKey: true
        hasDefault: false
      }>
    >(example)

    expect(example.description).toMatchInlineSnapshot(`
      {
        "defaultValue": undefined,
        "hasDefault": false,
        "notNull": true,
        "primaryKey": true,
        "type": "INTEGER",
      }
    `)
  })
})
