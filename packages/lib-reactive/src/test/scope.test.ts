import { describe, test } from "vitest"

import { setTimeout } from "node:timers/promises"

import { createScope } from "../scope"

describe("Scope", () => {
  test("should clean up on dispose in reverse order", async ({ expect }) => {
    const scope = createScope("test")
    const cleanups: string[] = []

    scope.onCleanup(() => {
      cleanups.push("a")
    })
    scope.onCleanup(async () => {
      await setTimeout(20)
      cleanups.push("b")
    })
    scope.onCleanup(() => {
      cleanups.push("c")
    })

    expect(cleanups).toEqual([])

    await scope.dispose()

    expect(cleanups).toEqual(["c", "b", "a"])
  })
})
