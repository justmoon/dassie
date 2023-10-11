import { describe, test, vi } from "vitest"

import { createActor, createReactor } from ".."

describe("createReactor", () => {
  test("should create a reactor", ({ expect }) => {
    const reactor = createReactor()
    expect(reactor).toBeTypeOf("object")
  })

  test("should run the root actor", ({ expect }) => {
    const behavior = vi.fn()
    const RootActor = () => createActor(behavior)

    createReactor(RootActor)

    expect(behavior).toHaveBeenCalledTimes(1)
  })

  test("should cleanup the root actor when reactor is disposed", async ({
    expect,
  }) => {
    const cleanup = vi.fn()
    const RootActor = () =>
      createActor((sig) => {
        sig.onCleanup(cleanup)
      })

    const reactor = createReactor(RootActor)

    expect(cleanup).toHaveBeenCalledTimes(0)

    await reactor.lifecycle.dispose()

    expect(cleanup).toHaveBeenCalledTimes(1)
  })
})
