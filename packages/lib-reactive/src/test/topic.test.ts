import { describe, test } from "vitest"

import { createTopic } from ".."

describe("createTopic", () => {
  test("should create a topic", ({ expect }) => {
    const topic = createTopic<boolean>()
    expect(topic).toBeTypeOf("object")
  })
})
