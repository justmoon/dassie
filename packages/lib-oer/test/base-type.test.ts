import { describe, test } from "vitest"

import { boolean } from "../src"

describe("base type", () => {
  describe("as a boolean", () => {
    const schema = boolean()

    test("should be an object", ({ expect }) => {
      expect(schema).toBeTypeOf("object")
    })

    describe("clone", () => {
      test("should return a copy", ({ expect }) => {
        const clone = schema.clone()
        expect(clone).toBeTypeOf("object")
        expect(clone.constructor).toEqual(schema.constructor)
      })
    })
  })
})
