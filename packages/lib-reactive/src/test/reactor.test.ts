import { describe, test, vi } from "vitest"

import { type Reactor, createActor, createReactor } from ".."

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

    await reactor.dispose()

    expect(cleanup).toHaveBeenCalledTimes(1)
  })

  test("should provide a base", ({ expect }) => {
    const reactor = createReactor(undefined, { foo: "bar" })
    expect(reactor.base).toEqual({ foo: "bar" })
  })

  test("should provide base to factories", ({ expect }) => {
    const factory = vi.fn((reactor: Reactor<{ foo: string }>) => {
      expect(reactor.base).toEqual({ foo: "bar" })

      return { bar: reactor.base.foo }
    })

    const reactor = createReactor(undefined, { foo: "bar" })

    const result = reactor.use(factory)

    expect(factory).toHaveBeenCalledOnce()
    expect(result).toEqual({ bar: "bar" })
  })
})
