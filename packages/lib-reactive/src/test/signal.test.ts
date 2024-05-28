import { describe, test, vi } from "vitest"

import { setTimeout } from "node:timers/promises"

import { createReactor, createSignal } from ".."

describe("createSignal", () => {
  test("should be able to create a signal", ({ expect }) => {
    const signal = createSignal("foo")

    expect(signal).toBeTypeOf("object")
  })

  test("should be able to read the current value", ({ expect }) => {
    const signal = createSignal("foo")

    expect(signal.read()).toBe("foo")
  })

  test("should be able to write the value", ({ expect }) => {
    const signal = createSignal("foo")

    signal.write("bar")

    expect(signal.read()).toBe("bar")
  })

  test("should be able to update the value", ({ expect }) => {
    const signal = createSignal("foo")

    signal.update((value) => value + "bar")

    expect(signal.read()).toBe("foobar")
  })

  test("should be able to subscribe to new values", async ({ expect }) => {
    const listener = vi.fn()
    const signal = createSignal("foo")

    const reactor = createReactor()

    signal.values.on(reactor, listener)

    expect(listener).toHaveBeenCalledTimes(0)

    signal.write("bar")

    await setTimeout()

    expect(listener).toHaveBeenCalledTimes(1)
  })
})
