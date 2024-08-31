import { describe, test } from "vitest"

import { setTimeout } from "node:timers/promises"

import { createLifecycleScope } from "../lifecycle"

describe("Lifecycle", () => {
  test("should clean up on dispose in reverse order", async ({ expect }) => {
    const scope = createLifecycleScope("test")
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
