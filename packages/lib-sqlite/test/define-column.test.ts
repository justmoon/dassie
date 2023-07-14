import { describe, test } from "vitest"

import { column } from "../src"

describe("column schema builder", () => {
  test("should start with a default column definition", ({ expect }) => {
    const example = column()

    expect(example.description).toMatchInlineSnapshot(`
      {
        "deserialize": [Function],
        "primaryKey": false,
        "required": false,
        "serialize": [Function],
        "type": "TEXT",
      }
    `)
  })

  test("should allow setting the type", ({ expect }) => {
    const example = column().type("INTEGER")

    expect(example.description).toMatchInlineSnapshot(`
      {
        "deserialize": [Function],
        "primaryKey": false,
        "required": false,
        "serialize": [Function],
        "type": "INTEGER",
      }
    `)
  })

  test("should allow setting the required flag", ({ expect }) => {
    const example = column().required()

    expect(example.description).toMatchInlineSnapshot(`
      {
        "deserialize": [Function],
        "primaryKey": false,
        "required": true,
        "serialize": [Function],
        "type": "TEXT",
      }
    `)
  })

  test("should allow setting the primary key flag", ({ expect }) => {
    const example = column().primaryKey()

    expect(example.description).toMatchInlineSnapshot(`
      {
        "deserialize": [Function],
        "primaryKey": true,
        "required": false,
        "serialize": [Function],
        "type": "TEXT",
      }
    `)
  })

  test("should allow setting the serialize function", ({ expect }) => {
    const example = column().serialize(String)

    expect(example.description).toMatchInlineSnapshot(`
      {
        "deserialize": [Function],
        "primaryKey": false,
        "required": false,
        "serialize": [Function],
        "type": "TEXT",
      }
    `)

    expect(example.description.serialize).toBe(String)
  })

  test("should allow setting the deserialize function", ({ expect }) => {
    const example = column().deserialize(Number)

    expect(example.description).toMatchInlineSnapshot(`
      {
        "deserialize": [Function],
        "primaryKey": false,
        "required": false,
        "serialize": [Function],
        "type": "TEXT",
      }
    `)
    expect(example.description.deserialize).toBe(Number)
  })

  test("should allow setting the type, required flag and primary key flag", ({
    expect,
  }) => {
    const example = column().type("INTEGER").required().primaryKey()

    expect(example.description).toMatchInlineSnapshot(`
      {
        "deserialize": [Function],
        "primaryKey": true,
        "required": true,
        "serialize": [Function],
        "type": "INTEGER",
      }
    `)
  })

  test("should allow setting the type, required flag, primary key flag, serialize function, and deserialize function", ({
    expect,
  }) => {
    const example = column()
      .type("INTEGER")
      .required()
      .primaryKey()
      .serialize(BigInt)
      .deserialize(Number)

    expect(example.description).toMatchInlineSnapshot(`
      {
        "deserialize": [Function],
        "primaryKey": true,
        "required": true,
        "serialize": [Function],
        "type": "INTEGER",
      }
    `)
  })
})
