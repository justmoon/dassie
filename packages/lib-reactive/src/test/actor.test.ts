import { produce } from "immer"
import { afterEach, beforeEach, describe, test, vi } from "vitest"

import { setTimeout } from "node:timers/promises"

import {
  type ActorContext,
  createActor,
  createReactor,
  createSignal,
  createTopic,
} from ".."

describe("createActor", () => {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["queueMicrotask"] })
  })
  afterEach(() => {
    vi.restoreAllMocks()
  })

  test("should create an actor", ({ expect }) => {
    const actor = createActor(() => ({}))

    expect(actor).toBeTypeOf("object")
  })

  describe("with synchronous behavior", () => {
    test("should re-run when tracked signals change", async ({ expect }) => {
      const reactor = createReactor()
      const signal = createSignal(1)

      const behavior = vi.fn(
        (sig: ActorContext) => sig.readAndTrack(signal) * 3,
      )

      const actor = createActor(behavior)

      expect(actor.result).toBeUndefined()
      expect(behavior).toHaveBeenCalledTimes(0)

      actor.run(reactor)
      expect(actor.result).toBe(3)
      expect(behavior).toHaveBeenCalledTimes(1)

      signal.write(2)
      expect(actor.result).toBe(3)
      expect(behavior).toHaveBeenCalledTimes(1)

      vi.runAllTicks()
      await setTimeout()
      expect(actor.result).toBe(6)
      expect(behavior).toHaveBeenCalledTimes(2)

      await reactor.dispose()
    })

    test("should not re-run if a tracked signal changes but the value is the same", async ({
      expect,
    }) => {
      const reactor = createReactor()
      const signal = createSignal(1)

      const behavior = vi.fn(
        (sig: ActorContext) => sig.readAndTrack(signal) * 3,
      )

      const actor = createActor(behavior)

      expect(actor.result).toBeUndefined()
      expect(behavior).toHaveBeenCalledTimes(0)

      actor.run(reactor)
      expect(actor.result).toBe(3)
      expect(behavior).toHaveBeenCalledTimes(1)

      signal.write(1)
      expect(actor.result).toBe(3)
      expect(behavior).toHaveBeenCalledTimes(1)

      vi.runAllTicks()
      await setTimeout()
      expect(actor.result).toBe(3)
      expect(behavior).toHaveBeenCalledTimes(1)

      await reactor.dispose()
    })

    test("should re-run if a tracked signal changes even if it changes back to the original value in the same tick", async ({
      expect,
    }) => {
      const reactor = createReactor()
      const signal = createSignal(1)

      const behavior = vi.fn(
        (sig: ActorContext) => sig.readAndTrack(signal) * 3,
      )

      const actor = createActor(behavior)

      expect(actor.result).toBeUndefined()
      expect(behavior).toHaveBeenCalledTimes(0)

      actor.run(reactor)
      expect(actor.result).toBe(3)
      expect(behavior).toHaveBeenCalledTimes(1)

      signal.write(2)
      expect(actor.result).toBe(3)
      expect(behavior).toHaveBeenCalledTimes(1)

      signal.write(1)
      expect(actor.result).toBe(3)
      expect(behavior).toHaveBeenCalledTimes(1)

      vi.runAllTicks()
      await setTimeout()
      expect(actor.result).toBe(3)
      expect(behavior).toHaveBeenCalledTimes(2)

      await reactor.dispose()
    })

    test("should call cleanup handler if reactor is disposed", async ({
      expect,
    }) => {
      const reactor = createReactor()

      const cleanup = vi.fn(() => {})
      const behavior = vi.fn((sig: ActorContext) => {
        sig.onCleanup(cleanup)
      })

      const actor = createActor(behavior)

      actor.run(reactor)
      expect(behavior).toHaveBeenCalledTimes(1)
      expect(cleanup).toHaveBeenCalledTimes(0)

      await reactor.dispose()
      expect(cleanup).toHaveBeenCalledTimes(1)
    })

    test("should call cleanup handler if actor re-runs", async ({ expect }) => {
      const reactor = createReactor()
      const signal = createSignal(1)

      const cleanup = vi.fn(() => {})

      const behavior = vi.fn((sig: ActorContext) => {
        sig.readAndTrack(signal)
        sig.onCleanup(cleanup)
      })

      const actor = createActor(behavior)

      actor.run(reactor)
      expect(behavior).toHaveBeenCalledTimes(1)
      expect(cleanup).toHaveBeenCalledTimes(0)

      signal.write(2)
      vi.runAllTicks()
      await setTimeout()
      expect(behavior).toHaveBeenCalledTimes(2)
      expect(cleanup).toHaveBeenCalledTimes(1)
    })

    test("should re-run actors that read another actor's result", async ({
      expect,
    }) => {
      const signal = createSignal(1)

      const behaviorA = vi.fn(
        (sig: ActorContext) => sig.readAndTrack(signal) * 3,
      )
      const ActorA = () => createActor(behaviorA)

      const behaviorB = vi.fn((sig: ActorContext) => {
        const value = sig.readAndTrack(ActorA)
        return value === undefined ? "foo" : value * 2
      })
      const ActorB = () => createActor(behaviorB)

      const RootActor = () =>
        createActor((sig) => {
          sig.run(ActorA)
          sig.run(ActorB)
        })

      const reactor = createReactor(RootActor)

      expect(behaviorA).toHaveBeenCalledTimes(1)
      expect(behaviorB).toHaveBeenCalledTimes(1)
      // When we synchronously access an actor's result, it will be undefined,
      // because the actor only runs on the next tick.
      expect(reactor.use(ActorB).result).toBe("foo")

      await setTimeout()
      expect(behaviorA).toHaveBeenCalledTimes(1)
      expect(behaviorB).toHaveBeenCalledTimes(2)
      expect(reactor.use(ActorB).result).toBe(6)

      signal.write(2)
      await setTimeout()

      expect(behaviorA).toHaveBeenCalledTimes(2)
      expect(behaviorB).toHaveBeenCalledTimes(3)
      expect(reactor.use(ActorB).result).toBe(12)
    })

    test("should only re-run actor if its selected data changes", async ({
      expect,
    }) => {
      const signal = createSignal({ a: 1, b: 2 })

      const behavior = vi.fn(
        (sig: ActorContext) => sig.readAndTrack(signal, ({ a }) => a) * 3,
      )

      const Actor = () => createActor(behavior)

      const RootActor = () =>
        createActor((sig) => {
          sig.run(Actor)
        })

      createReactor(RootActor)

      expect(behavior).toHaveBeenCalledTimes(1)

      signal.update(
        produce((draft) => {
          draft.b = 3
        }),
      )

      await setTimeout()

      expect(behavior).toHaveBeenCalledTimes(1)

      signal.update(
        produce((draft) => {
          draft.a = 2
        }),
      )

      await setTimeout()

      expect(behavior).toHaveBeenCalledTimes(2)
    })

    test("should only re-run actor if selected keys change", async ({
      expect,
    }) => {
      const signal = createSignal({ a: 1, b: 2 })

      const behavior = vi.fn((sig: ActorContext) => {
        const { a } = sig.readKeysAndTrack(signal, ["a"])

        return a * 3
      })

      const Actor = () => createActor(behavior)

      const RootActor = () =>
        createActor((sig) => {
          sig.run(Actor)
        })

      createReactor(RootActor)

      expect(behavior).toHaveBeenCalledTimes(1)

      signal.update(
        produce((draft) => {
          draft.b = 3
        }),
      )

      await setTimeout()

      expect(behavior).toHaveBeenCalledTimes(1)

      signal.update(
        produce((draft) => {
          draft.a = 2
        }),
      )

      await setTimeout()

      expect(behavior).toHaveBeenCalledTimes(2)
    })

    test("should re-run actor on subscribed topic message", async ({
      expect,
    }) => {
      const topic = createTopic()

      const behavior = vi.fn((sig: ActorContext) => {
        sig.subscribe(topic)
      })

      const Actor = () => createActor(behavior)

      createReactor(Actor)

      expect(behavior).toHaveBeenCalledTimes(1)

      topic.emit()

      await setTimeout()

      expect(behavior).toHaveBeenCalledTimes(2)
    })
  })

  describe("with API", () => {
    test("should be able to 'tell' API handler", async ({ expect }) => {
      const apiHandler = vi.fn()
      const Actor = () =>
        createActor(() => {
          return {
            greet: apiHandler,
          }
        })

      const RootActor = () =>
        createActor((sig) => {
          sig.run(Actor)
        })

      const reactor = createReactor(RootActor)

      const actor = reactor.use(Actor)

      expect(apiHandler).toHaveBeenCalledTimes(0)

      actor.api.greet.tell()

      await setTimeout()

      expect(apiHandler).toHaveBeenCalledTimes(1)
    })

    test("should be able to 'ask' API handler", async ({ expect }) => {
      const apiHandler = vi.fn(() => "hello")
      const Actor = () =>
        createActor(() => {
          return {
            greet: apiHandler,
          }
        })

      const RootActor = () =>
        createActor((sig) => {
          sig.run(Actor)
        })

      const reactor = createReactor(RootActor)

      const actor = reactor.use(Actor)

      expect(apiHandler).toHaveBeenCalledTimes(0)

      const response = await actor.api.greet.ask()

      expect(response).toBe("hello")
      expect(apiHandler).toHaveBeenCalledTimes(1)
    })

    test("should be able to 'tell' API handler even before actor is running", async ({
      expect,
    }) => {
      const apiHandler = vi.fn()
      const Actor = () =>
        createActor(() => {
          return {
            greet: apiHandler,
          }
        })

      const enableSignal = createSignal(false)

      const RootActor = () =>
        createActor((sig) => {
          if (sig.readAndTrack(enableSignal)) {
            sig.run(Actor)
          }
        })

      const reactor = createReactor(RootActor)

      const actor = reactor.use(Actor)

      expect(apiHandler).toHaveBeenCalledTimes(0)

      actor.api.greet.tell()
      await setTimeout()

      expect(apiHandler).toHaveBeenCalledTimes(0)

      enableSignal.write(true)
      await setTimeout()

      expect(apiHandler).toHaveBeenCalledTimes(1)
    })

    test("should be able to 'ask' API handler even before actor is running", async ({
      expect,
    }) => {
      const apiHandler = vi.fn(() => "hello")
      const Actor = () =>
        createActor(() => {
          return {
            greet: apiHandler,
          }
        })

      const enableSignal = createSignal(false)

      const RootActor = () =>
        createActor((sig) => {
          if (sig.readAndTrack(enableSignal)) {
            sig.run(Actor)
          }
        })

      const reactor = createReactor(RootActor)

      const actor = reactor.use(Actor)

      expect(apiHandler).toHaveBeenCalledTimes(0)

      const responsePromise = actor.api.greet.ask()
      await setTimeout()

      expect(apiHandler).toHaveBeenCalledTimes(0)

      enableSignal.write(true)
      const response = await responsePromise

      expect(response).toBe("hello")
      expect(apiHandler).toHaveBeenCalledTimes(1)
    })
  })

  describe("base", () => {
    test("should be able to access base context", ({ expect }) => {
      const behavior = vi.fn()

      const RootActor = () => createActor(behavior)

      createReactor(RootActor, { foo: "bar" })

      expect(behavior).toHaveBeenCalledTimes(1)
      expect(behavior).toHaveBeenCalledWith(
        expect.objectContaining({
          base: { foo: "bar" },
        }),
      )
    })

    test("should be able to extend base context", ({ expect }) => {
      const behavior = vi.fn()

      const Actor = () => createActor(behavior)

      const RootActor = () =>
        createActor((sig) => {
          sig.withBase({ foo: "bar" }).run(Actor)
        })

      createReactor(RootActor)

      expect(behavior).toHaveBeenCalledTimes(1)
      expect(behavior).toHaveBeenCalledWith(
        expect.objectContaining({
          base: { foo: "bar" },
        }),
      )
    })

    test("should be able to extend base context twice", ({ expect }) => {
      const innerBehavior = vi.fn()
      const middleBehavior = vi.fn()

      const Actor = () =>
        createActor((sig: ActorContext<{ foo: string; bar: string }>) => {
          innerBehavior(sig)
        })

      const MiddleActor = () =>
        createActor((sig: ActorContext<{ foo: string }>) => {
          middleBehavior(sig)
          sig
            .withBase({
              bar: "baz",
            })
            .run(Actor)
        })

      const RootActor = () =>
        createActor((sig) => {
          sig.withBase({ foo: "bar" }).run(MiddleActor)
        })

      createReactor(RootActor)

      expect(middleBehavior).toHaveBeenCalledTimes(1)
      expect(middleBehavior).toHaveBeenCalledWith(
        expect.objectContaining({
          base: { foo: "bar" },
        }),
      )

      expect(innerBehavior).toHaveBeenCalledTimes(1)
      expect(innerBehavior).toHaveBeenCalledWith(
        expect.objectContaining({
          base: { foo: "bar", bar: "baz" },
        }),
      )
    })

    test("should not modify reactor context", ({ expect }) => {
      const behavior = vi.fn(
        (sig: ActorContext<{ foo: string; bar: string }>) => {
          expect(sig.base).toEqual({ foo: "bar", bar: "baz" })
        },
      )

      const Actor = () => createActor(behavior)

      const RootActor = () =>
        createActor((sig: ActorContext<{ foo: string }>) => {
          sig.withBase({ bar: "baz" }).run(Actor)
        })

      const reactor = createReactor(RootActor, { foo: "bar" })

      expect(behavior).toHaveBeenCalledTimes(1)
      expect(reactor.base).toEqual({ foo: "bar" })
    })

    test("should not modify own context", ({ expect }) => {
      const checkContext = vi.fn((sig: ActorContext<{ foo: string }>) => {
        expect(sig.base).toEqual({ foo: "bar" })
      })

      const Actor = () => createActor(() => {})

      const MiddleActor = () =>
        createActor((sig: ActorContext<{ foo: string }>) => {
          checkContext(sig)
          sig
            .withBase({
              bar: "baz",
            })
            .run(Actor)
          checkContext(sig)
        })

      const RootActor = () =>
        createActor((sig) => {
          sig.withBase({ foo: "bar" }).run(MiddleActor)
        })

      createReactor(RootActor)

      expect(checkContext).toHaveBeenCalledTimes(2)
    })

    test("should not modify own context on future runs", async ({ expect }) => {
      const checkContext = vi.fn((sig: ActorContext<{ foo: string }>) => {
        expect(sig.base).toEqual({ foo: "bar" })
      })

      const topic = createTopic()

      const Actor = () => createActor(() => {})

      const MiddleActor = () =>
        createActor((sig: ActorContext<{ foo: string }>) => {
          sig.subscribe(topic)
          checkContext(sig)
          sig
            .withBase({
              bar: "baz",
            })
            .run(Actor)
        })

      const RootActor = () =>
        createActor((sig) => {
          sig.withBase({ foo: "bar" }).run(MiddleActor)
        })

      createReactor(RootActor)

      expect(checkContext).toHaveBeenCalledTimes(1)

      topic.emit()

      await setTimeout()

      expect(checkContext).toHaveBeenCalledTimes(2)
    })
  })
})
